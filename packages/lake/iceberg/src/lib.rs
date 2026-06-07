//! The Iceberg corpus lake: the durable, replayable log under the multi-source
//! search corpus (issue #752), succeeding the full-file-overwrite parquet log.
//!
//! One table (`corpus.documents`) holds an **append-only revision log** of
//! [`Document`] observations: each reconcile pass appends only the documents
//! that are new or changed (`op = upsert`) plus tombstones for the ones that
//! left the writer's desired state (`op = delete`). Current state is a
//! per-slice fold ordered by each slice's committed `version` counter: an
//! `external_id` is live while any slice's latest op for it is an upsert, so
//! one host's tombstone cannot erase a record another slice still observes,
//! and a wall-clock step on a writer cannot reorder its operations. Replay
//! and incremental catch-up share one discipline, the **snapshot cursor**
//! ([`added_since`]).
//!
//! Both halves live in this one crate — unlike the `sink-parquet` /
//! `source-parquet` pair, where the object layout is the whole contract, the
//! write and read halves here share the table's schema, codec, catalog
//! connection, and fold, so splitting them would only duplicate that core.
//!
//! - **Write half**: [`IcebergReconciler`] implements
//!   [`source_meta::Reconciler`]. Each pass diffs the writer's *slice* (its
//!   `host`, optional `user`) against the desired set and appends the delta.
//!   An empty desired set is treated as "source absent", never as "delete
//!   everything" (matching `sink-parquet`), so a transiently empty read cannot
//!   tombstone a corpus.
//! - **Read half**: [`read_state`] folds the whole log into the current
//!   per-source document sets (full rebuilds, including sources whose records
//!   are all tombstoned, so a rebuild can also garbage-collect); [`added_since`]
//!   walks only the snapshots a cursor has not seen (steady-state view
//!   catch-up). Compaction (`Replace` snapshots, e.g. R2 Data Catalog's
//!   managed compaction) rewrites files without adding rows, so the cursor
//!   walk follows `Append` snapshots only.
//!
//! Production is a REST catalog (Cloudflare R2 Data Catalog) over S3-compatible
//! storage ([`Config::connect`]); tests run the same code against iceberg's
//! in-memory catalog.

#![forbid(unsafe_code)]

mod codec;
mod error;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub use codec::{OP_DELETE, OP_UPSERT};
pub use error::{Error, Result};
// Re-exported so consumers can hold a connected lake without depending on the
// iceberg crate (and its arrow tree) directly.
pub use iceberg::{Catalog, TableIdent};

use futures::TryStreamExt as _;
use iceberg::expr::{Predicate, Reference};
use iceberg::io::FileIO;
use iceberg::spec::{
    DataContentType, DataFile, Datum, ManifestContentType, ManifestStatus, Operation,
};
use iceberg::table::Table;
use iceberg::transaction::{ApplyTransactionAction as _, Transaction};
use iceberg::writer::base_writer::data_file_writer::DataFileWriterBuilder;
use iceberg::writer::file_writer::ParquetWriterBuilder;
use iceberg::writer::file_writer::location_generator::{
    DefaultFileNameGenerator, DefaultLocationGenerator,
};
use iceberg::writer::file_writer::rolling_writer::RollingFileWriterBuilder;
use iceberg::writer::{IcebergWriter as _, IcebergWriterBuilder as _};
use iceberg::{CatalogBuilder as _, ErrorKind, NamespaceIdent, TableCreation};
use iceberg_catalog_rest::{
    REST_CATALOG_PROP_URI, REST_CATALOG_PROP_WAREHOUSE, RestCatalogBuilder,
};
use iceberg_storage_opendal::OpenDalStorageFactory;
use parquet_57::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet_57::file::properties::WriterProperties;
use snafu::{OptionExt as _, ResultExt as _};
use source_meta::{Document, Reconciler, Source};

use crate::codec::{LakeRow, Op, Slice};
use crate::error::{
    ClockBeforeEpochSnafu, ClockSnafu, CommitSnafu, ConnectSnafu, CursorNotFoundSnafu,
    DecodeFileSnafu, EnsureTableSnafu, LoadTableSnafu, ParseFileSnafu, ReadFileSnafu, SchemaSnafu,
    ScanSnafu, WriteSnafu,
};

/// The lake's Iceberg namespace.
pub const NAMESPACE: &str = "corpus";
/// The lake's table name within [`NAMESPACE`].
pub const TABLE: &str = "documents";

/// How many times a conflicted append commit is retried (each attempt reloads
/// the table and re-applies the already-written data files).
const COMMIT_ATTEMPTS: u32 = 5;

/// Connection settings for the production REST catalog (R2 Data Catalog).
#[derive(Debug, Clone)]
pub struct Config {
    /// Catalog URI (R2: `https://catalog.cloudflarestorage.com/<account>/<bucket>`).
    pub uri: String,
    /// Warehouse name the catalog serves (R2: `<account>_<bucket>`).
    pub warehouse: String,
    /// Bearer token for the catalog REST API.
    pub token: Option<String>,
    /// S3 endpoint for the data plane (R2: the account endpoint). `None` lets
    /// the catalog's own storage config (if vended) or AWS defaults apply.
    pub s3_endpoint: Option<String>,
    /// S3 region label (`auto` for R2).
    pub s3_region: String,
}

impl Config {
    /// Connect the REST catalog with an S3 data plane. S3 credentials come
    /// from the environment (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`),
    /// the same convention as the parquet sink.
    ///
    /// # Errors
    /// Returns an error if the catalog handshake fails.
    pub async fn connect(&self) -> Result<Arc<dyn Catalog>> {
        let mut props = HashMap::from([
            (REST_CATALOG_PROP_URI.to_owned(), self.uri.clone()),
            (REST_CATALOG_PROP_WAREHOUSE.to_owned(), self.warehouse.clone()),
            (iceberg::io::S3_REGION.to_owned(), self.s3_region.clone()),
        ]);
        if let Some(token) = &self.token {
            props.insert("token".to_owned(), token.clone());
        }
        if let Some(endpoint) = &self.s3_endpoint {
            props.insert(iceberg::io::S3_ENDPOINT.to_owned(), endpoint.clone());
        }
        for (env, key) in [
            ("AWS_ACCESS_KEY_ID", iceberg::io::S3_ACCESS_KEY_ID),
            ("AWS_SECRET_ACCESS_KEY", iceberg::io::S3_SECRET_ACCESS_KEY),
        ] {
            if let Ok(value) = std::env::var(env) {
                props.insert(key.to_owned(), value);
            }
        }
        let catalog = RestCatalogBuilder::default()
            .with_storage_factory(Arc::new(OpenDalStorageFactory::S3 {
                configured_scheme: "s3".to_owned(),
                customized_credential_load: None,
            }))
            .load("lake", props)
            .await
            .context(ConnectSnafu { uri: self.uri.clone() })?;
        Ok(Arc::new(catalog))
    }
}

/// Create the `corpus.documents` table if absent and return its identifier.
/// Race-safe: a concurrent host winning the create is fine (`AlreadyExists`
/// from either step is success).
///
/// # Errors
/// Returns an error if the namespace or table cannot be created or checked.
pub async fn ensure_table(catalog: &dyn Catalog) -> Result<TableIdent> {
    let ns = NamespaceIdent::new(NAMESPACE.to_owned());
    let ident = TableIdent::new(ns.clone(), TABLE.to_owned());
    match catalog.create_namespace(&ns, HashMap::new()).await {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NamespaceAlreadyExists => {}
        Err(error) => return Err(error).context(EnsureTableSnafu { table: ident.to_string() }),
    }
    let creation =
        TableCreation::builder().name(TABLE.to_owned()).schema(codec::table_schema()?).build();
    match catalog.create_table(&ns, creation).await {
        Ok(_) => Ok(ident),
        Err(error) if error.kind() == ErrorKind::TableAlreadyExists => Ok(ident),
        Err(error) => Err(error).context(EnsureTableSnafu { table: ident.to_string() }),
    }
}

/// Outcome of one lake reconcile pass for a source.
#[derive(Debug, Clone, Copy)]
pub struct Report {
    /// Documents appended as new or changed observations.
    pub upserts: usize,
    /// Tombstones appended for documents that left the slice's desired state.
    pub deletes: usize,
    /// Whether the pass appended nothing (desired state already converged, or
    /// the desired set was empty).
    pub skipped: bool,
}

/// Reconciles a source's documents into the lake as an appended delta.
///
/// Holds the writer's *slice* identity: tombstones are computed against this
/// `host`/`user`'s own previous observations only, so host A never deletes
/// what host B still observes (the per-user fleet path derives per-account
/// reconcilers via [`IcebergReconciler::with_user`], mirroring the parquet
/// sink's per-user prefixes).
pub struct IcebergReconciler {
    /// The connected catalog.
    catalog: Arc<dyn Catalog>,
    /// The lake table.
    ident: TableIdent,
    /// The writing host.
    host: String,
    /// The account, for per-user sources.
    user: Option<String>,
}

impl IcebergReconciler {
    /// A reconciler writing host-level observations (no `user` scope).
    #[must_use]
    pub fn new(catalog: Arc<dyn Catalog>, ident: TableIdent, host: impl Into<String>) -> Self {
        Self { catalog, ident, host: host.into(), user: None }
    }

    /// The same connection, scoped to one account's slice.
    #[must_use]
    pub fn with_user(&self, user: impl Into<String>) -> Self {
        Self {
            catalog: Arc::clone(&self.catalog),
            ident: self.ident.clone(),
            host: self.host.clone(),
            user: Some(user.into()),
        }
    }

    /// The predicate selecting this slice's rows for one source.
    fn slice_filter(&self, source: &Source) -> Predicate {
        let user = self.user.as_ref().map_or_else(
            || Reference::new("user").is_null(),
            |user| Reference::new("user").equal_to(Datum::string(user)),
        );
        Reference::new("source")
            .equal_to(Datum::string(source.as_str()))
            .and(Reference::new("host").equal_to(Datum::string(&self.host)))
            .and(user)
    }
}

impl Reconciler for IcebergReconciler {
    type Report = Report;
    type Error = Error;

    /// Diff `documents` against this slice's live state and append the delta:
    /// upserts for new or changed documents, tombstones for vanished ones.
    /// Converged state appends nothing (no empty snapshots). An empty
    /// `documents` is a skip, never a mass tombstone.
    async fn reconcile(&self, source: &Source, documents: &[Document]) -> Result<Report> {
        if documents.is_empty() {
            return Ok(Report { upserts: 0, deletes: 0, skipped: true });
        }
        let table = load_table(self.catalog.as_ref(), &self.ident).await?;

        // The slice's live state: latest observation per id, minus tombstones.
        // The filter pins one slice, so each id folds to exactly one row.
        let rows =
            scan_rows(&table, Some(self.slice_filter(source)), &codec::STATE_COLUMNS, false)
                .await?;
        // The slice's next committed revision: its previous maximum plus one.
        // `version`, not wall clock, is what orders this slice's operations in
        // the fold, so a clock step between runs cannot reorder them.
        let version = rows.iter().map(|row| row.version).max().unwrap_or(0) + 1;
        let mut live: HashMap<String, Option<String>> = HashMap::new();
        for (id, slice_rows) in codec::fold_slices(rows) {
            if let Some(row) = codec::live_winner(slice_rows) {
                live.insert(id, row.content_hash);
            }
        }

        // The delta. A live record with no stored hash cannot happen from this
        // writer, but is treated as changed (re-observe) rather than trusted.
        let upserts: Vec<&Document> = documents
            .iter()
            .filter(|document| {
                !matches!(
                    live.get(&document.external_id),
                    Some(Some(hash)) if *hash == document.content_hash
                )
            })
            .collect();
        let desired: BTreeSet<&str> =
            documents.iter().map(|document| document.external_id.as_str()).collect();
        let deletes: BTreeSet<&str> = live
            .keys()
            .map(String::as_str)
            .filter(|id| !desired.contains(id))
            .collect();
        if upserts.is_empty() && deletes.is_empty() {
            return Ok(Report { upserts: 0, deletes: 0, skipped: true });
        }
        let deletes: Vec<&str> = deletes.into_iter().collect();

        let arrow_schema = Arc::new(
            iceberg::arrow::schema_to_arrow_schema(table.metadata().current_schema())
                .context(SchemaSnafu)?,
        );
        let batch = codec::encode_batch(
            &arrow_schema,
            source,
            Slice { host: &self.host, user: self.user.as_deref() },
            now_ms()?,
            version,
            &upserts,
            &deletes,
        )?;
        let files = write_batch(&table, batch).await?;
        commit_files(self.catalog.as_ref(), &self.ident, files).await?;
        Ok(Report { upserts: upserts.len(), deletes: deletes.len(), skipped: false })
    }
}

/// The lake's folded current state, for the rebuild/replace consume paths.
#[derive(Debug, Default)]
pub struct LakeState {
    /// Every source that has ever written a row, mapped to its live documents
    /// (sorted by `external_id`). A source whose records are all tombstoned is
    /// present with an empty set, so a rebuild can still garbage-collect its
    /// view records.
    pub sources: BTreeMap<String, Vec<Document>>,
}

/// Fold the whole log into [`LakeState`] — the full rebuild primitive:
/// replaying it into a view (uploads for what is here, deletes for what is
/// not, per source) reproduces current state.
///
/// Liveness is per slice: an id stays while any slice's latest op for it is
/// an upsert, so one host's tombstone never erases a record another slice
/// still observes.
///
/// # Errors
/// Returns an error if the table cannot be loaded or scanned, or a row is
/// malformed.
pub async fn read_state(catalog: &dyn Catalog, ident: &TableIdent) -> Result<LakeState> {
    let table = load_table(catalog, ident).await?;
    let rows = scan_rows(&table, None, &codec::CODEC_COLUMNS, true).await?;
    let mut sources: BTreeMap<String, Vec<Document>> = BTreeMap::new();
    for row in &rows {
        sources.entry(row.source.clone()).or_default();
    }
    for slice_rows in codec::fold_slices(rows).into_values() {
        if let Some(row) = codec::live_winner(slice_rows) {
            let documents = sources.entry(row.source.clone()).or_default();
            documents.push(codec::document_from_row(row)?);
        }
    }
    for documents in sources.values_mut() {
        documents.sort_by(|a, b| a.external_id.cmp(&b.external_id));
    }
    Ok(LakeState { sources })
}

/// The table's current snapshot id, the cursor a caught-up consumer stores.
/// `None` for a table with no commits yet.
///
/// # Errors
/// Returns an error if the table cannot be loaded.
pub async fn current_snapshot_id(catalog: &dyn Catalog, ident: &TableIdent) -> Result<Option<i64>> {
    let table = load_table(catalog, ident).await?;
    Ok(table.metadata().current_snapshot().map(|snapshot| snapshot.snapshot_id()))
}

/// The changes a cursor has not seen, folded per slice (a slice's later op on
/// an id supersedes its earlier one; liveness spans slices).
#[derive(Debug)]
pub struct Delta {
    /// Documents observed new or changed since the cursor, plus the surviving
    /// replica's document for any id one slice tombstoned while another still
    /// holds it (so the view converges to [`read_state`]).
    pub upserts: Vec<Document>,
    /// Ids tombstoned since the cursor by their last holder: verified against
    /// the whole table's state, not just the delta, so a slice-scoped
    /// tombstone never deletes a record live in another slice.
    pub deletes: Vec<String>,
    /// The snapshot this delta is current to; store it as the next cursor.
    pub to_snapshot: Option<i64>,
}

/// Walk the snapshots appended after `cursor` and fold their rows into a
/// [`Delta`] — the steady-state incremental read.
///
/// Only `Append` snapshots are followed: `Replace` snapshots are compaction
/// (R2 Data Catalog's managed compaction emits these) and rewrite existing
/// rows into new files, so following them would re-deliver the whole table
/// after every compaction. Within each followed snapshot, only manifests it
/// added are read — manifest files are immutable and carried forward, so an
/// unfiltered walk would also re-deliver every old file.
///
/// An id whose post-cursor rows end in tombstones only proves those slices
/// let go of it; a slice untouched since the cursor may still hold it live.
/// Such candidates are checked against the whole table's state (a cheap
/// no-payload scan, only on deltas that contain tombstones) before they reach
/// `deletes`, and a surviving replica is re-emitted as an upsert so the view
/// converges to [`read_state`].
///
/// # Errors
/// Returns [`Error::CursorNotFound`] when `cursor` is no longer in table
/// metadata (snapshot expiration) — the caller falls back to [`read_state`] —
/// and other errors when the walk or a data file read fails.
pub async fn added_since(
    catalog: &dyn Catalog,
    ident: &TableIdent,
    cursor: i64,
) -> Result<Delta> {
    let table = load_table(catalog, ident).await?;
    let meta = table.metadata();
    let cursor_seq = meta
        .snapshots()
        .find(|snapshot| snapshot.snapshot_id() == cursor)
        .map(|snapshot| snapshot.sequence_number())
        .context(CursorNotFoundSnafu { snapshot: cursor })?;

    let mut rows = Vec::new();
    let appends = meta.snapshots().filter(|snapshot| {
        snapshot.sequence_number() > cursor_seq
            && snapshot.summary().operation == Operation::Append
    });
    for snapshot in appends {
        let list = snapshot
            .load_manifest_list(table.file_io(), meta)
            .await
            .context(ScanSnafu { stage: "manifest list" })?;
        for manifest_file in list.entries() {
            if manifest_file.content != ManifestContentType::Data
                || manifest_file.added_snapshot_id != snapshot.snapshot_id()
            {
                continue;
            }
            let manifest = manifest_file
                .load_manifest(table.file_io())
                .await
                .context(ScanSnafu { stage: "manifest" })?;
            for entry in manifest.entries() {
                if entry.status() == ManifestStatus::Added
                    && entry.data_file().content_type() == DataContentType::Data
                {
                    read_data_file(table.file_io(), entry.data_file().file_path(), &mut rows)
                        .await?;
                }
            }
        }
    }

    let mut upserts = Vec::new();
    let mut candidates = Vec::new();
    for (id, slice_rows) in codec::fold_slices(rows) {
        match codec::live_winner(slice_rows) {
            Some(row) => upserts.push(codec::document_from_row(row)?),
            None => candidates.push(id),
        }
    }

    let mut deletes = Vec::new();
    if !candidates.is_empty() {
        let state = scan_rows(&table, None, &codec::STATE_COLUMNS, false).await?;
        let global = codec::fold_slices(state);
        let mut survivors = Vec::new();
        for id in candidates {
            let live = global
                .get(&id)
                .is_some_and(|rows| rows.iter().any(|row| row.op == Op::Upsert));
            if live {
                survivors.push(id);
            } else {
                deletes.push(id);
            }
        }
        upserts.extend(read_documents_by_id(&table, &survivors).await?);
    }

    upserts.sort_by(|a, b| a.external_id.cmp(&b.external_id));
    deletes.sort();
    Ok(Delta { upserts, deletes, to_snapshot: meta.current_snapshot().map(|s| s.snapshot_id()) })
}

/// Read the current live document of each id — the survivor fetch: ids whose
/// post-cursor rows are all tombstones but which another slice still holds
/// live. Scoped to those ids by predicate, so it reads payloads for a handful
/// of records, not the table.
async fn read_documents_by_id(table: &Table, ids: &[String]) -> Result<Vec<Document>> {
    let Some(filter) = ids
        .iter()
        .map(|id| Reference::new("external_id").equal_to(Datum::string(id)))
        .reduce(Predicate::or)
    else {
        return Ok(Vec::new());
    };
    let rows = scan_rows(table, Some(filter), &codec::CODEC_COLUMNS, true).await?;
    codec::fold_slices(rows)
        .into_values()
        .filter_map(codec::live_winner)
        .map(codec::document_from_row)
        .collect()
}

/// Load the lake table, with a typed error naming it.
async fn load_table(catalog: &dyn Catalog, ident: &TableIdent) -> Result<Table> {
    catalog.load_table(ident).await.context(LoadTableSnafu { table: ident.to_string() })
}

/// Scan rows with an optional filter and a column projection, decoding into
/// [`LakeRow`]s (`with_payload` per [`codec::rows_from_batch`]).
async fn scan_rows(
    table: &Table,
    filter: Option<Predicate>,
    columns: &[&str],
    with_payload: bool,
) -> Result<Vec<LakeRow>> {
    // A table with no commits yet has nothing to scan (and no snapshot for the
    // scan planner to anchor on).
    if table.metadata().current_snapshot().is_none() {
        return Ok(Vec::new());
    }
    let mut builder = table.scan().select(columns.iter().copied());
    if let Some(predicate) = filter {
        builder = builder.with_filter(predicate);
    }
    let stream = builder
        .build()
        .context(ScanSnafu { stage: "build" })?
        .to_arrow()
        .await
        .context(ScanSnafu { stage: "open" })?;
    let batches: Vec<arrow_array::RecordBatch> =
        stream.try_collect().await.context(ScanSnafu { stage: "read" })?;
    let mut rows = Vec::new();
    for batch in &batches {
        codec::rows_from_batch(batch, with_payload, &mut rows)?;
    }
    Ok(rows)
}

/// Read one data file's rows directly through the table's `FileIO` (the
/// incremental walk reads files the scan planner has no task list for).
async fn read_data_file(file_io: &FileIO, path: &str, out: &mut Vec<LakeRow>) -> Result<()> {
    let input = file_io.new_input(path).context(ReadFileSnafu { path })?;
    let bytes = input.read().await.context(ReadFileSnafu { path })?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes)
        .context(ParseFileSnafu { path })?
        .build()
        .context(ParseFileSnafu { path })?;
    for batch in reader {
        let batch = batch.context(DecodeFileSnafu { path })?;
        codec::rows_from_batch(&batch, true, out)?;
    }
    Ok(())
}

/// Write one record batch as data files, named uniquely per pass (a reused
/// name is rejected by the table as already-referenced).
async fn write_batch(
    table: &Table,
    batch: arrow_array::RecordBatch,
) -> Result<Vec<DataFile>> {
    let location_gen = DefaultLocationGenerator::new(table.metadata().clone())
        .context(WriteSnafu { stage: "location" })?;
    let name_gen = DefaultFileNameGenerator::new(
        "corpus".to_owned(),
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
    let mut writer = DataFileWriterBuilder::new(rolling)
        .build(None)
        .await
        .context(WriteSnafu { stage: "build" })?;
    writer.write(batch).await.context(WriteSnafu { stage: "write" })?;
    writer.close().await.context(WriteSnafu { stage: "close" })
}

/// Commit already-written data files, retrying commit conflicts (concurrent
/// fleet hosts appending) by reloading the table and re-applying the same
/// files — data files are snapshot-independent, so they are reusable across
/// attempts.
async fn commit_files(
    catalog: &dyn Catalog,
    ident: &TableIdent,
    files: Vec<DataFile>,
) -> Result<()> {
    let mut attempt = 1;
    loop {
        let table = load_table(catalog, ident).await?;
        let tx = Transaction::new(&table);
        let action = tx.fast_append().add_data_files(files.clone());
        let tx = action.apply(tx).context(CommitSnafu { attempts: attempt })?;
        match tx.commit(catalog).await {
            Ok(_) => return Ok(()),
            Err(error)
                if error.kind() == ErrorKind::CatalogCommitConflicts
                    && attempt < COMMIT_ATTEMPTS =>
            {
                attempt += 1;
            }
            Err(error) => return Err(error).context(CommitSnafu { attempts: attempt }),
        }
    }
}

/// Current time as epoch milliseconds, strictly monotonic within this process
/// so two passes in the same millisecond still order correctly in the fold.
fn now_ms() -> Result<i64> {
    static LAST: Mutex<i64> = Mutex::new(0);
    let elapsed =
        SystemTime::now().duration_since(UNIX_EPOCH).context(ClockBeforeEpochSnafu)?;
    let now = i64::try_from(elapsed.as_millis()).context(ClockSnafu)?;
    let stamped = {
        let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let stamped = now.max(*last + 1);
        *last = stamped;
        stamped
    };
    Ok(stamped)
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "tests assert observable lake outcomes")]

    use std::collections::HashMap;
    use std::sync::Arc;

    use iceberg::memory::{MEMORY_CATALOG_WAREHOUSE, MemoryCatalogBuilder};
    use iceberg::{Catalog, CatalogBuilder as _, TableIdent};
    use serde_json::json;
    use snafu::IntoError as _;
    use source_meta::{Document, Reconciler as _, Source};

    use super::{
        IcebergReconciler, LakeState, added_since, current_snapshot_id, ensure_table, read_state,
    };

    /// A memory-catalog lake for one test.
    struct TestLake {
        catalog: Arc<dyn Catalog>,
        ident: TableIdent,
        /// Held for its `Drop`: the warehouse directory lives inside.
        _dir: tempfile::TempDir,
    }

    async fn lake() -> TestLake {
        let dir = tempfile::tempdir().expect("tempdir");
        let warehouse = format!("file://{}", dir.path().display());
        let catalog = MemoryCatalogBuilder::default()
            .load("lake", HashMap::from([(MEMORY_CATALOG_WAREHOUSE.to_owned(), warehouse)]))
            .await
            .expect("memory catalog");
        let catalog: Arc<dyn Catalog> = Arc::new(catalog);
        let ident = ensure_table(catalog.as_ref()).await.expect("ensure table");
        TestLake { catalog, ident, _dir: dir }
    }

    fn doc_in(source: &str, id: &str, body: &str) -> Document {
        let content_hash = source_meta::hash_body(body.as_bytes());
        Document {
            external_id: id.to_owned(),
            file_name: id.to_owned(),
            mime: "text/plain",
            body: body.as_bytes().to_vec(),
            meta_json: json!({
                "source": source,
                "external_id": id,
                "content_hash": content_hash,
                "title": format!("title {id}"),
                "timestamp": 100,
            }),
            content_hash,
        }
    }

    fn doc(id: &str, body: &str) -> Document {
        doc_in("test", id, body)
    }

    /// Flatten a [`LakeState`] into its live documents (source-major, id-sorted
    /// within a source) — the shape most assertions want.
    fn live_docs(state: LakeState) -> Vec<Document> {
        state.sources.into_values().flatten().collect()
    }

    #[tokio::test]
    async fn reconcile_appends_then_skips_then_reads_back() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let sink = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-1");
        let source = Source::new("test");
        let docs = vec![doc("a", "alpha"), doc("b", "beta")];

        let first = sink.reconcile(&source, &docs).await.expect("first");
        assert_eq!((first.upserts, first.deletes, first.skipped), (2, 0, false));

        // Converged state appends nothing — no empty snapshots.
        let snapshot = current_snapshot_id(catalog.as_ref(), &ident)
            .await
            .expect("snapshot")
            .expect("one commit");
        let second = sink.reconcile(&source, &docs).await.expect("second");
        assert!(second.skipped);
        assert_eq!(
            current_snapshot_id(catalog.as_ref(), &ident).await.expect("snapshot"),
            Some(snapshot),
            "a skipped pass must not commit"
        );

        // Full read-back round-trips the documents (source-parquet parity:
        // file_name = external_id, plain-text mime, meta_json intact).
        let all = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].external_id, "a");
        assert_eq!(all[0].body, b"alpha");
        assert_eq!(all[0].file_name, "a");
        assert_eq!(all[0].mime, "text/plain");
        assert_eq!(all[0].meta_json["title"], "title a");
        assert_eq!(all[1].external_id, "b");
    }

    #[tokio::test]
    async fn change_and_vanish_append_upsert_and_tombstone() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let sink = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-1");
        let source = Source::new("test");
        sink.reconcile(&source, &[doc("a", "alpha"), doc("b", "beta")]).await.expect("seed");

        // `a` changes, `b` vanishes from the desired state.
        let report =
            sink.reconcile(&source, &[doc("a", "alpha EDITED")]).await.expect("delta");
        assert_eq!((report.upserts, report.deletes), (1, 1));

        let all = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        assert_eq!(all.len(), 1, "the tombstoned document must fold away");
        assert_eq!(all[0].external_id, "a");
        assert_eq!(all[0].body, b"alpha EDITED");
    }

    #[tokio::test]
    async fn empty_desired_state_never_mass_tombstones() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let sink = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-1");
        let source = Source::new("test");
        sink.reconcile(&source, &[doc("a", "alpha")]).await.expect("seed");

        // A transiently empty read (unreadable export, fresh account) must be
        // a skip, not a corpus wipe.
        let report = sink.reconcile(&source, &[]).await.expect("empty");
        assert!(report.skipped);
        let all = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        assert_eq!(all.len(), 1, "the document must survive an empty pass");
    }

    #[tokio::test]
    async fn slices_tombstone_independently() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let source = Source::new("test");
        let host1 = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-1");
        let host2 = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-2");
        host1.reconcile(&source, &[doc("one", "from host 1")]).await.expect("host1 seed");
        host2.reconcile(&source, &[doc("two", "from host 2")]).await.expect("host2 seed");

        // host-1's document vanishes; host-2's slice must be untouched.
        let report = host1.reconcile(&source, &[doc("one-b", "new")]).await.expect("host1 delta");
        assert_eq!(report.deletes, 1);
        let all = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        let ids: Vec<&str> = all.iter().map(|d| d.external_id.as_str()).collect();
        assert_eq!(ids, ["one-b", "two"], "host-2's document must survive host-1's tombstone");

        // The per-user derivation scopes the same way.
        let alice = host1.with_user("alice");
        alice.reconcile(&source, &[doc("alice-1", "hers")]).await.expect("alice seed");
        let report = host1.reconcile(&source, &[doc("one-b", "new")]).await.expect("host1 again");
        assert!(report.skipped, "host-level slice must not see alice's rows");
    }

    /// Commit one crafted batch onto an explicit (possibly stale) table
    /// handle, bypassing the reconciler's reload-per-attempt loop and its
    /// clock/version assignment — the construction the conflict tests and the
    /// ordering tests share.
    async fn commit_on(
        catalog: &dyn Catalog,
        table: &iceberg::table::Table,
        source: &Source,
        observed_at: i64,
        version: i64,
        upserts: &[&Document],
        deletes: &[&str],
    ) -> super::Result<()> {
        use iceberg::transaction::{ApplyTransactionAction as _, Transaction};
        let schema = Arc::new(
            iceberg::arrow::schema_to_arrow_schema(table.metadata().current_schema())
                .expect("arrow schema"),
        );
        let batch = crate::codec::encode_batch(
            &schema,
            source,
            crate::codec::Slice { host: "host-1", user: None },
            observed_at,
            version,
            upserts,
            deletes,
        )
        .expect("batch");
        let files = super::write_batch(table, batch).await.expect("write files");
        let tx = Transaction::new(table);
        let tx = tx.fast_append().add_data_files(files).apply(tx).expect("apply");
        tx.commit(catalog).await.map(|_| ()).map_err(|error| {
            super::error::CommitSnafu { attempts: 1_u32 }.into_error(error)
        })
    }

    #[tokio::test]
    async fn stale_base_append_merges_without_losing_either_commit() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let sink = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-1");
        let source = Source::new("test");
        sink.reconcile(&source, &[doc("a", "alpha")]).await.expect("seed");

        // Hold a stale handle while another commit advances the table.
        let stale = catalog.load_table(&ident).await.expect("stale handle");
        sink.reconcile(&source, &[doc("a", "alpha"), doc("b", "beta")]).await.expect("advance");

        // Appends carry no snapshot-ref requirement, so a commit from the
        // stale base MERGES rather than conflicts: that is the lost-update
        // safety the fleet's concurrent writers rely on. (A REST server that
        // CASes the metadata pointer surfaces the same race as HTTP 409 →
        // CatalogCommitConflicts, which commit_files retries; that leg is
        // exercised in rest_round_trip_via_env against a real server.)
        let gamma = doc("c", "gamma");
        commit_on(catalog.as_ref(), &stale, &source, 1, 1, &[&gamma], &[])
            .await
            .expect("a stale-base append must merge");
        let all = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        let ids: Vec<&str> = all.iter().map(|d| d.external_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"], "no commit may be lost to the race");
    }

    #[tokio::test]
    async fn concurrent_writers_all_land() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let source = Source::new("test");
        let mut handles = Vec::new();
        for i in 0..4 {
            let catalog = Arc::clone(&catalog);
            let ident = ident.clone();
            let source = source.clone();
            handles.push(tokio::spawn(async move {
                let sink = IcebergReconciler::new(catalog, ident, format!("host-{i}"));
                sink.reconcile(&source, &[doc(&format!("doc-{i}"), "body")]).await
            }));
        }
        for handle in handles {
            let report = handle.await.expect("join").expect("reconcile");
            assert!(!report.skipped);
        }
        let all = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        assert_eq!(all.len(), 4, "every concurrent writer's document must land");
    }

    /// The same code against a live REST catalog, env-configured. One test,
    /// three backends: the memory catalog covers the suite above,
    /// `fixture/rest-fixture.sh` stands up the apache/iceberg-rest-fixture +
    /// `MinIO` pair locally, and R2 Data Catalog staging uses the same
    /// variables with Cloudflare values (see fixture/README.md).
    #[tokio::test]
    #[ignore = "needs a live REST catalog: run fixture/rest-fixture.sh, or set LAKE_TEST_* for R2"]
    async fn rest_round_trip_via_env() {
        let uri = std::env::var("LAKE_TEST_CATALOG_URI")
            .expect("LAKE_TEST_CATALOG_URI must be set; see fixture/README.md");
        let config = super::Config {
            uri,
            warehouse: std::env::var("LAKE_TEST_WAREHOUSE").unwrap_or_default(),
            token: std::env::var("LAKE_TEST_CATALOG_TOKEN").ok(),
            s3_endpoint: std::env::var("LAKE_TEST_S3_ENDPOINT").ok(),
            s3_region: std::env::var("LAKE_TEST_S3_REGION").unwrap_or_else(|_| "auto".to_owned()),
        };
        let catalog = config.connect().await.expect("connect");
        let ident = ensure_table(catalog.as_ref()).await.expect("ensure table");

        // Unique ids and source tag per run: repeated runs share the catalog.
        let tag = format!("resttest-{}", uuid::Uuid::new_v4());
        let source = Source::new(tag.clone());
        let id = |suffix: &str| format!("{tag}:{suffix}");
        let sink = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "rest-host");

        // Reconcile, converge, change + tombstone — the memory-catalog suite's
        // arc, through the production REST + S3 wiring.
        let seed = vec![doc_in(&tag, &id("r1"), "one"), doc_in(&tag, &id("r2"), "two")];
        let first = sink.reconcile(&source, &seed).await.expect("seed");
        assert_eq!((first.upserts, first.deletes, first.skipped), (2, 0, false));
        assert!(sink.reconcile(&source, &seed).await.expect("converged").skipped);

        let cursor = current_snapshot_id(catalog.as_ref(), &ident)
            .await
            .expect("snapshot")
            .expect("committed");
        let changed = vec![doc_in(&tag, &id("r1"), "one EDITED")];
        let delta_report = sink.reconcile(&source, &changed).await.expect("delta");
        assert_eq!((delta_report.upserts, delta_report.deletes), (1, 1));

        let mine: Vec<Document> = live_docs(
            read_state(catalog.as_ref(), &ident).await.expect("read_state"),
        )
        .into_iter()
        .filter(|d| d.meta_json["source"] == tag.as_str())
        .collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].body, b"one EDITED");

        let delta = added_since(catalog.as_ref(), &ident, cursor).await.expect("added_since");
        assert!(delta.deletes.contains(&id("r2")), "the tombstone must arrive via the cursor");

        // The stale-base leg, against a real server. Two acceptable behaviors
        // exist, and which one a backend exhibits is exactly what this records:
        // an Iceberg-aware server rebases the append (merge, like the memory
        // catalog), while a metadata-pointer-CAS server answers HTTP 409,
        // which must map to the kind commit_files retries on. Anything else
        // is a contract break.
        let stale = catalog.load_table(&ident).await.expect("stale handle");
        sink.reconcile(&source, &[doc_in(&tag, &id("r3"), "three")]).await.expect("advance");
        let c1 = doc_in(&tag, &id("c1"), "x");
        match commit_on(catalog.as_ref(), &stale, &source, 1, 1, &[&c1], &[]).await {
            Ok(()) => {
                eprintln!("[rest_round_trip] stale-base append: backend MERGES (no retry needed)");
                let mine: Vec<Document> = live_docs(
                    read_state(catalog.as_ref(), &ident).await.expect("read_state after merge"),
                )
                .into_iter()
                .filter(|d| d.meta_json["source"] == tag.as_str())
                .collect();
                assert!(
                    mine.iter().any(|d| d.external_id == id("c1")),
                    "the merged append must not be lost"
                );
            }
            Err(super::Error::Commit { source: inner, .. }) => {
                eprintln!("[rest_round_trip] stale-base append: backend CASes (409, retryable)");
                assert_eq!(
                    inner.kind(),
                    iceberg::ErrorKind::CatalogCommitConflicts,
                    "a commit rejection must carry the kind the retry loop matches"
                );
            }
            Err(other) => panic!("unexpected stale-base failure shape: {other:?}"),
        }
    }

    #[tokio::test]
    async fn snapshot_cursor_sees_only_later_appends() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let sink = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-1");
        let source = Source::new("test");
        sink.reconcile(&source, &[doc("a", "alpha")]).await.expect("first");
        let cursor = current_snapshot_id(catalog.as_ref(), &ident)
            .await
            .expect("snapshot")
            .expect("one commit");

        sink.reconcile(&source, &[doc("a", "alpha"), doc("b", "beta")]).await.expect("second");
        sink.reconcile(&source, &[doc("b", "beta")]).await.expect("third tombstones a");

        let delta = added_since(catalog.as_ref(), &ident, cursor).await.expect("delta");
        let upsert_ids: Vec<&str> =
            delta.upserts.iter().map(|d| d.external_id.as_str()).collect();
        assert_eq!(upsert_ids, ["b"], "only the post-cursor document arrives");
        assert_eq!(delta.deletes, ["a"], "the later tombstone wins over a's earlier upsert");
        assert_eq!(
            delta.to_snapshot,
            current_snapshot_id(catalog.as_ref(), &ident).await.expect("snapshot"),
            "the delta reports the cursor to store next"
        );

        // A caught-up cursor yields an empty delta.
        let caught_up = delta.to_snapshot.expect("snapshot");
        let empty = added_since(catalog.as_ref(), &ident, caught_up).await.expect("empty delta");
        assert!(empty.upserts.is_empty() && empty.deletes.is_empty());

        // An expired/unknown cursor is the typed full-rescan signal.
        let missing = added_since(catalog.as_ref(), &ident, 0).await;
        assert!(
            matches!(missing, Err(super::Error::CursorNotFound { snapshot: 0, .. })),
            "an unknown cursor must demand a full rescan, got {missing:?}"
        );
    }

    #[tokio::test]
    async fn version_orders_a_slice_not_wall_clock() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let source = Source::new("test");
        let alpha = doc("a", "alpha");

        // A writer whose clock stepped backward between runs (NTP, reboot):
        // the tombstone at version 2 carries a SMALLER observed_at than the
        // upsert it supersedes. Committed version order must decide.
        let table = catalog.load_table(&ident).await.expect("table");
        commit_on(catalog.as_ref(), &table, &source, 1_000, 1, &[&alpha], &[])
            .await
            .expect("upsert");
        let cursor = current_snapshot_id(catalog.as_ref(), &ident)
            .await
            .expect("snapshot")
            .expect("one commit");
        let table = catalog.load_table(&ident).await.expect("reload");
        commit_on(catalog.as_ref(), &table, &source, 10, 2, &[], &["a"])
            .await
            .expect("tombstone");

        let docs = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        assert!(docs.is_empty(), "the version-2 tombstone must win over the older wall clock");
        let delta = added_since(catalog.as_ref(), &ident, cursor).await.expect("delta");
        assert_eq!(delta.deletes, ["a"], "the cursor read must apply the same order");

        // And back: a later re-observation with an even older clock revives it.
        let table = catalog.load_table(&ident).await.expect("reload");
        let revived = doc("a", "alpha revived");
        commit_on(catalog.as_ref(), &table, &source, 5, 3, &[&revived], &[])
            .await
            .expect("revive");
        let docs = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].body, b"alpha revived");
    }

    #[tokio::test]
    async fn cross_slice_tombstone_keeps_the_other_slices_record() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let source = Source::new("test");
        let host1 = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-1");
        let host2 = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-2");
        // The same record observed by two slices (synced shell history, the
        // same repo indexed on two hosts).
        host1.reconcile(&source, &[doc("x", "shared")]).await.expect("host1 seed");
        host2.reconcile(&source, &[doc("x", "shared")]).await.expect("host2 seed");
        let cursor = current_snapshot_id(catalog.as_ref(), &ident)
            .await
            .expect("snapshot")
            .expect("committed");

        // host-1 lets go of x; host-2 still observes it.
        host1.reconcile(&source, &[doc("y", "new")]).await.expect("host1 delta");
        let all = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        let ids: Vec<&str> = all.iter().map(|d| d.external_id.as_str()).collect();
        assert_eq!(ids, ["x", "y"], "host-2's replica must keep x alive");

        let delta = added_since(catalog.as_ref(), &ident, cursor).await.expect("delta");
        assert_eq!(
            delta.deletes,
            Vec::<String>::new(),
            "a slice-scoped tombstone must not delete a record live in another slice"
        );
        let upsert_ids: Vec<&str> = delta.upserts.iter().map(|d| d.external_id.as_str()).collect();
        assert_eq!(upsert_ids, ["x", "y"], "the surviving replica is re-emitted to converge");

        // Once the last holder lets go, the delete goes through.
        let cursor = delta.to_snapshot.expect("snapshot");
        host2.reconcile(&source, &[doc("z", "other")]).await.expect("host2 delta");
        let delta = added_since(catalog.as_ref(), &ident, cursor).await.expect("second delta");
        assert_eq!(delta.deletes, ["x"], "the last holder's tombstone must delete");
        let all = live_docs(read_state(catalog.as_ref(), &ident).await.expect("read_state"));
        let ids: Vec<&str> = all.iter().map(|d| d.external_id.as_str()).collect();
        assert_eq!(ids, ["y", "z"]);
    }

    #[tokio::test]
    async fn read_state_keeps_fully_tombstoned_sources_for_gc() {
        let TestLake { catalog, ident, _dir } = lake().await;
        let source = Source::new("test");
        let sink = IcebergReconciler::new(Arc::clone(&catalog), ident.clone(), "host-1");
        sink.reconcile(&source, &[doc("a", "alpha")]).await.expect("seed");
        // Tombstone the source's only record (crafted directly: the reconciler
        // never appends a bare tombstone, but manual surgery can leave a
        // source fully dead, and the rebuild must still GC its view records).
        let table = catalog.load_table(&ident).await.expect("table");
        commit_on(catalog.as_ref(), &table, &source, 2, 2, &[], &["a"])
            .await
            .expect("tombstone");

        let state = read_state(catalog.as_ref(), &ident).await.expect("read_state");
        assert_eq!(
            state.sources.get("test").map(Vec::len),
            Some(0),
            "a fully tombstoned source must stay listed so a rebuild can GC it"
        );
    }
}
