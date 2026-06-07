//! Phase 0.4 spike for issue #752: exercise iceberg-rust against a catalog and
//! answer, with running code, the questions that gate the `sink-iceberg` /
//! `source-iceberg` design:
//!
//!   S1  create namespace + table through the catalog
//!   S2  append a batch, commit, observe snapshot 1
//!   S3  append a second batch, observe snapshot 2
//!   S4  full scan returns all rows
//!   S5  incremental read between two snapshots (the snapshot cursor):
//!       walk snapshots newer than the cursor and collect ADDED data files
//!       from their manifests — the `source-iceberg` primitive
//!   S6  two concurrent appenders: both commits land (with retry), no rows lost
//!
//! Backend is chosen by env:
//!   default              -> in-memory catalog + tempdir warehouse (local, free)
//!   SPIKE_CATALOG=rest   -> REST catalog; set SPIKE_CATALOG_URI, SPIKE_WAREHOUSE,
//!                           SPIKE_CATALOG_TOKEN (R2 Data Catalog: uri is
//!                           https://catalog.cloudflarestorage.com/<account>/<bucket>)

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use arrow_array::{ArrayRef, RecordBatch, StringArray};
use iceberg::memory::{MEMORY_CATALOG_WAREHOUSE, MemoryCatalogBuilder};
use iceberg::spec::{
    DataContentType, ManifestStatus, NestedField, Operation, PrimitiveType, Schema, Type,
};
use iceberg::table::Table;
use iceberg::transaction::{ApplyTransactionAction, Transaction};
use iceberg::writer::base_writer::data_file_writer::DataFileWriterBuilder;
use iceberg::writer::file_writer::ParquetWriterBuilder;
use iceberg::writer::file_writer::location_generator::{
    DefaultFileNameGenerator, DefaultLocationGenerator,
};
use iceberg::writer::file_writer::rolling_writer::RollingFileWriterBuilder;
use iceberg::writer::{IcebergWriter, IcebergWriterBuilder};
use iceberg::io::{S3_ACCESS_KEY_ID, S3_ENDPOINT, S3_REGION, S3_SECRET_ACCESS_KEY};
use iceberg::{Catalog, CatalogBuilder, NamespaceIdent, TableCreation, TableIdent};
use iceberg_catalog_rest::{REST_CATALOG_PROP_URI, REST_CATALOG_PROP_WAREHOUSE, RestCatalogBuilder};
use iceberg_storage_opendal::OpenDalStorageFactory;
use parquet::file::properties::WriterProperties;

const NAMESPACE: &str = "spike";
const TABLE: &str = "documents";

fn corpus_schema() -> Result<Schema> {
    // A slice of the real corpus schema (sink-parquet's 9 columns + op), enough
    // to be representative without dragging the whole Document shape in.
    Ok(Schema::builder()
        .with_schema_id(0)
        .with_fields(vec![
            NestedField::required(1, "external_id", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(2, "source", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(3, "content_hash", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(4, "body", Type::Primitive(PrimitiveType::String)).into(),
            NestedField::required(5, "op", Type::Primitive(PrimitiveType::String)).into(),
        ])
        .build()?)
}

fn batch(table: &Table, ids: &[&str], op: &str) -> Result<RecordBatch> {
    // Arrow schema derived from the table schema so parquet field ids line up.
    let arrow_schema =
        Arc::new(iceberg::arrow::schema_to_arrow_schema(table.metadata().current_schema())?);
    let n = ids.len();
    let col = |f: &dyn Fn(usize) -> String| -> ArrayRef {
        Arc::new(StringArray::from((0..n).map(f).collect::<Vec<_>>()))
    };
    Ok(RecordBatch::try_new(
        arrow_schema,
        vec![
            col(&|i| ids[i].to_string()),
            col(&|_| "spike".to_string()),
            col(&|i| format!("sha256:{i:064x}")),
            col(&|i| format!("body of {}", ids[i])),
            col(&|_| op.to_string()),
        ],
    )?)
}

/// Append one batch to the table and commit through the catalog; returns the
/// new snapshot id. This is the exact shape `sink-iceberg` will use.
async fn append_batch(catalog: &dyn Catalog, table: &Table, rows: RecordBatch) -> Result<i64> {
    let location_gen = DefaultLocationGenerator::new(table.metadata().clone())?;
    // SPIKE FINDING: the suffix must be unique per run — without it the
    // generator emits `spike-00000.parquet` every time and the second commit is
    // rejected with "Cannot add files that are already referenced by table".
    // sink-iceberg must do the same (uuid per sync run).
    let name_gen = DefaultFileNameGenerator::new(
        "spike".into(),
        Some(uuid::Uuid::new_v4().to_string()),
        iceberg::spec::DataFileFormat::Parquet,
    );
    let parquet_writer = ParquetWriterBuilder::new(
        WriterProperties::default(),
        table.metadata().current_schema().clone(),
    );
    let rolling = RollingFileWriterBuilder::new_with_default_file_size(
        parquet_writer,
        table.file_io().clone(),
        location_gen,
        name_gen,
    );
    let mut writer = DataFileWriterBuilder::new(rolling).build(None).await?;
    writer.write(rows).await?;
    let files = writer.close().await?;

    let tx = Transaction::new(table);
    let append = tx.fast_append().add_data_files(files);
    let tx = append.apply(tx)?;
    let updated = tx.commit(catalog).await.context("commit append")?;
    let snap = updated
        .metadata()
        .current_snapshot()
        .context("no current snapshot after append")?
        .snapshot_id();
    Ok(snap)
}

/// S5: the snapshot-cursor read. Collect data files ADDED by snapshots strictly
/// newer than `cursor` (ancestry walk, oldest first). Returns file paths.
async fn added_files_since(table: &Table, cursor: i64) -> Result<Vec<String>> {
    let meta = table.metadata();
    // Snapshots are listed in metadata order; filter to those after the cursor
    // by sequence number (commit order), which survives compaction snapshots.
    let cursor_seq = meta
        .snapshots()
        .find(|s| s.snapshot_id() == cursor)
        .map(|s| s.sequence_number())
        .context("cursor snapshot not found (expired?) — caller must full-rescan")?;
    let mut added = Vec::new();
    // Only Append snapshots carry new corpus rows. Replace snapshots are
    // compaction (R2 Data Catalog's managed compaction emits these): they
    // rewrite existing rows into new files, and following them would make the
    // cursor re-deliver the whole table after every compaction.
    let appends = meta
        .snapshots()
        .filter(|s| s.sequence_number() > cursor_seq && s.summary().operation == Operation::Append);
    for snap in appends {
        let list = snap.load_manifest_list(table.file_io(), meta).await?;
        for mf in list.entries() {
            if mf.content != iceberg::spec::ManifestContentType::Data {
                continue; // delete manifests are not corpus rows
            }
            // SPIKE FINDING: manifest files are immutable and carried forward
            // between snapshots, so a later snapshot's manifest list still
            // contains earlier manifests whose entries read as Added. Without
            // this filter the cursor walk re-reads every old file each run.
            if mf.added_snapshot_id != snap.snapshot_id() {
                continue;
            }
            let manifest = mf.load_manifest(table.file_io()).await?;
            for entry in manifest.entries() {
                if entry.status() == ManifestStatus::Added
                    && entry.data_file().content_type() == DataContentType::Data
                {
                    added.push(entry.data_file().file_path().to_string());
                }
            }
        }
    }
    Ok(added)
}

async fn count_rows(table: &Table) -> Result<usize> {
    use futures::TryStreamExt;
    let stream = table.scan().build()?.to_arrow().await?;
    let batches: Vec<RecordBatch> = stream.try_collect().await?;
    Ok(batches.iter().map(RecordBatch::num_rows).sum())
}

/// S6: two writers race; each retries on commit conflict by reloading the table.
async fn concurrent_append(
    catalog: Arc<dyn Catalog>,
    ident: TableIdent,
    ids: Vec<String>,
    op: &'static str,
) -> Result<u32> {
    let mut retries = 0;
    loop {
        let table = catalog.load_table(&ident).await?;
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let rows = batch(&table, &id_refs, op)?;
        match append_batch(catalog.as_ref(), &table, rows).await {
            Ok(_) => return Ok(retries),
            Err(e) if retries < 5 => {
                eprintln!("  [s6] commit conflict, retrying: {e:#}");
                retries += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

async fn build_catalog() -> Result<(Arc<dyn Catalog>, Option<tempfile::TempDir>)> {
    match std::env::var("SPIKE_CATALOG").as_deref() {
        Ok("rest") => {
            let uri = std::env::var("SPIKE_CATALOG_URI").context("SPIKE_CATALOG_URI")?;
            let warehouse = std::env::var("SPIKE_WAREHOUSE").context("SPIKE_WAREHOUSE")?;
            let token = std::env::var("SPIKE_CATALOG_TOKEN").context("SPIKE_CATALOG_TOKEN")?;
            let mut props = HashMap::from([
                (REST_CATALOG_PROP_URI.to_string(), uri),
                (REST_CATALOG_PROP_WAREHOUSE.to_string(), warehouse),
                ("token".to_string(), token),
                (S3_REGION.to_string(), "auto".to_string()),
            ]);
            // R2's S3 data plane. The catalog's /config response may vend these;
            // explicit env wins for the spike so both paths can be tested.
            for (env, key) in [
                ("SPIKE_S3_ENDPOINT", S3_ENDPOINT),
                ("SPIKE_S3_ACCESS_KEY_ID", S3_ACCESS_KEY_ID),
                ("SPIKE_S3_SECRET_ACCESS_KEY", S3_SECRET_ACCESS_KEY),
            ] {
                if let Ok(v) = std::env::var(env) {
                    props.insert(key.to_string(), v);
                }
            }
            let catalog = RestCatalogBuilder::default()
                .with_storage_factory(Arc::new(OpenDalStorageFactory::S3 {
                    configured_scheme: "s3".to_string(),
                    customized_credential_load: None,
                }))
                .load("r2", props)
                .await?;
            Ok((Arc::new(catalog), None))
        }
        _ => {
            let dir = tempfile::tempdir()?;
            let warehouse = format!("file://{}", dir.path().display());
            let catalog = MemoryCatalogBuilder::default()
                .load(
                    "spike",
                    HashMap::from([(MEMORY_CATALOG_WAREHOUSE.to_string(), warehouse)]),
                )
                .await?;
            Ok((Arc::new(catalog), Some(dir)))
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let (catalog, _tmp) = build_catalog().await?;
    let ns = NamespaceIdent::new(NAMESPACE.to_string());
    let ident = TableIdent::new(ns.clone(), TABLE.to_string());

    // S1: namespace + table.
    if !catalog.namespace_exists(&ns).await? {
        catalog.create_namespace(&ns, HashMap::new()).await?;
    }
    if catalog.table_exists(&ident).await? {
        bail!(
            "S1 FAIL  {NAMESPACE}.{TABLE} already exists; delete it explicitly before rerunning the spike"
        );
    }
    let creation = TableCreation::builder()
        .name(TABLE.to_string())
        .schema(corpus_schema()?)
        .build();
    let table = catalog.create_table(&ns, creation).await?;
    println!("S1 PASS  created {}.{} at {}", NAMESPACE, TABLE, table.metadata().location());

    // S2: first append.
    let rows_a = batch(&table, &["doc-1", "doc-2"], "upsert")?;
    let snap1 = append_batch(catalog.as_ref(), &table, rows_a).await?;
    println!("S2 PASS  snapshot {snap1} after first append");

    // S3: second append (reload to pick up snap1).
    let table = catalog.load_table(&ident).await?;
    let rows_b = batch(&table, &["doc-3", "doc-2"], "upsert")?;
    let snap2 = append_batch(catalog.as_ref(), &table, rows_b).await?;
    println!("S3 PASS  snapshot {snap2} after second append");

    // S4: full scan sees all 4 rows.
    let table = catalog.load_table(&ident).await?;
    let total = count_rows(&table).await?;
    if total != 4 {
        bail!("S4 FAIL  expected 4 rows, scanned {total}");
    }
    println!("S4 PASS  full scan = {total} rows");

    // S5: incremental — files added strictly after snap1 must be exactly the
    // second append's single file, none of the first's.
    let added = added_files_since(&table, snap1).await?;
    if added.len() != 1 {
        bail!("S5 FAIL  expected exactly 1 file after cursor {snap1}, got {}: {added:?}", added.len());
    }
    println!("S5 PASS  cursor {snap1} -> {} added file(s): {added:?}", added.len());

    // S6: concurrent appenders.
    let a = tokio::spawn(concurrent_append(
        Arc::clone(&catalog),
        ident.clone(),
        vec!["c-1".into(), "c-2".into()],
        "upsert",
    ));
    let b = tokio::spawn(concurrent_append(
        Arc::clone(&catalog),
        ident.clone(),
        vec!["c-3".into(), "c-4".into()],
        "upsert",
    ));
    let (ra, rb) = (a.await??, b.await??);
    let table = catalog.load_table(&ident).await?;
    let total = count_rows(&table).await?;
    if total != 8 {
        bail!("S6 FAIL  expected 8 rows after concurrent appends, scanned {total}");
    }
    println!("S6 PASS  both writers landed (retries: {ra} + {rb}), total {total} rows");

    println!("\nALL SCENARIOS PASS");
    Ok(())
}
