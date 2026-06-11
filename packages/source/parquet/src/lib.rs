//! Read the S3/R2 parquet corpus log written by `sink-parquet` back into search
//! [`Document`]s.
//!
//! This is the consumer half of the parquet corpus log. The `sink-parquet`
//! sink writes one flat parquet file per source at
//! `<prefix>/source=<source>/data.parquet`, with a sibling
//! `<prefix>/source=<source>/_manifest.json`; this crate lists that prefix,
//! reads each `data.parquet`, and reconstructs the [`Document`]s so the indexer
//! can rebuild the Mixedbread search index from the log. This is the
//! materialized-view-over-a-log model (issue #736): the Parquet log is the
//! append-only source of truth, and the Mixedbread index is one view replayed
//! from it.
//!
//! # What is read
//! The sink's `meta_json` column already holds the FULL metadata object as a
//! JSON string, so a [`Document`] is reconstructed from just four columns:
//! `external_id`, `content_hash`, `body`, and `meta_json`. The other columns
//! (`source`, `title`, `url`, `host`, `timestamp`) are projections out of
//! `meta_json` for queryability, so they are ignored on read. Only objects whose
//! key ends in `/data.parquet` are parsed; the `_manifest.json` sidecar and any
//! other object under the prefix are skipped.
//!
//! Known limitation: this lists the whole prefix and materializes every row each
//! run. An incremental cursor (snapshot diffs, or an Iceberg upgrade) is a future
//! refinement, not a correctness issue.

#![forbid(unsafe_code)]

use arrow::array::{Array as _, StringArray};
use arrow::record_batch::RecordBatch;
use futures::stream::StreamExt as _;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt as _};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use snafu::{OptionExt as _, ResultExt as _, Snafu};
use source_meta::Document;

/// The object name `sink-parquet` writes for each source's data file. Only
/// objects with this suffix are parsed; the `_manifest.json` sidecar and any
/// other key under the prefix are skipped.
const DATA_SUFFIX: &str = "/data.parquet";

/// The four columns a [`Document`] is reconstructed from. The rest of
/// `sink-parquet`'s schema is a projection out of `meta_json`, so it is ignored
/// on read.
const COL_EXTERNAL_ID: &str = "external_id";
const COL_CONTENT_HASH: &str = "content_hash";
const COL_BODY: &str = "body";
const COL_META_JSON: &str = "meta_json";

/// Connection and layout for reading the parquet corpus log.
#[derive(Debug, Clone)]
pub struct Config {
    /// Bucket the parquet sink writes to.
    pub bucket: String,
    /// S3 endpoint URL. `None` uses AWS S3; for a self-hosted store pass its endpoint.
    pub endpoint: Option<String>,
    /// Region (`auto` for non-AWS stores).
    pub region: String,
    /// Key prefix under the bucket to read parquet objects from.
    pub prefix: String,
}

/// Failures from the parquet reader.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum Error {
    /// The S3 client could not be built from the config and environment.
    #[snafu(display("failed to build the S3 client for bucket {bucket}"))]
    BuildStore {
        /// Bucket the client targeted.
        bucket: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// Listing the prefix failed.
    #[snafu(display("failed to list parquet objects under {prefix}"))]
    List {
        /// Prefix being listed.
        prefix: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// An object could not be read.
    #[snafu(display("failed to read parquet object {key}"))]
    Get {
        /// Object key.
        key: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// A parquet object could not be opened (bad footer/metadata).
    #[snafu(display("failed to open parquet object {key}"))]
    Parquet {
        /// Object key.
        key: String,
        /// Underlying parquet error.
        source: parquet::errors::ParquetError,
    },
    /// A parquet object's record batches could not be decoded.
    #[snafu(display("failed to decode parquet object {key}"))]
    Decode {
        /// Object key.
        key: String,
        /// Underlying arrow error.
        source: arrow::error::ArrowError,
    },
    /// A required column was absent from a parquet object's schema.
    #[snafu(display("parquet object {key} is missing the required column {column}"))]
    MissingColumn {
        /// Object key.
        key: String,
        /// Name of the absent column.
        column: &'static str,
    },
    /// A required column was present but not the expected `Utf8` type.
    #[snafu(display("parquet object {key} column {column} is not a string column"))]
    ColumnType {
        /// Object key.
        key: String,
        /// Name of the mis-typed column.
        column: &'static str,
    },
    /// A required column held a null at some row. The sink writes these columns
    /// non-nullable, so a null is a malformed log; surface it as a typed error
    /// rather than reconstructing a document from an arbitrary default.
    #[snafu(display("parquet object {key} column {column} is null at row {row}"))]
    NullValue {
        /// Object key.
        key: String,
        /// Name of the column with the null cell.
        column: &'static str,
        /// Row index of the null cell.
        row: usize,
    },
    /// A row's `meta_json` string did not parse as JSON.
    #[snafu(display("failed to parse meta_json in parquet object {key}"))]
    MetaJson {
        /// Object key.
        key: String,
        /// Underlying serde error.
        source: serde_json::Error,
    },
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Read every parquet object under the configured prefix and reconstruct the
/// corpus documents they carry.
///
/// # Errors
/// Returns an error if the client cannot be built, the prefix cannot be listed,
/// or any object cannot be read, decoded, or reconstructed.
pub async fn read_documents(config: &Config) -> Result<Vec<Document>> {
    let store = build_store(config)?;
    read_from_store(&store, &config.prefix).await
}

/// The store-agnostic core of [`read_documents`], so tests can drive an
/// in-memory store.
///
/// Lists `<prefix>` and, for each object whose key ends in `/data.parquet`,
/// fetches the bytes and parses them. The `_manifest.json` sidecar and any other
/// object under the prefix are skipped.
///
/// # Errors
/// Returns an error if the store cannot be listed, an object cannot be read, or a
/// data file cannot be parsed as the corpus parquet schema.
pub async fn read_from_store(store: &dyn ObjectStore, prefix: &str) -> Result<Vec<Document>> {
    let mut documents = Vec::new();
    let mut listing = store.list(Some(&ObjectPath::from(prefix)));
    while let Some(entry) = listing.next().await {
        let meta = entry.context(ListSnafu { prefix })?;
        let key = meta.location.to_string();
        // Only the per-source data files are corpus parquet; the `_manifest.json`
        // sidecar (and anything else) under the prefix is not, so skip it.
        if !key.ends_with(DATA_SUFFIX) {
            continue;
        }
        let result = store
            .get(&meta.location)
            .await
            .context(GetSnafu { key: key.clone() })?;
        let bytes = result
            .bytes()
            .await
            .context(GetSnafu { key: key.clone() })?;
        parse_parquet(bytes, &key, &mut documents)?;
    }
    Ok(documents)
}

/// A corpus slice: one `data.parquet` and the `host=/user=/source=` identity
/// parsed from its key.
///
/// The lake fold needs each slice's origin host and user, which
/// [`read_documents`] flattens away.
pub struct Slice {
    /// The `host=` segment, when present in the key.
    pub host: Option<String>,
    /// The `user=` segment, when present (host-level sources have none).
    pub user: Option<String>,
    /// The `source=` segment (the corpus tag).
    pub source: String,
    /// The documents this slice's data file carries.
    pub documents: Vec<Document>,
}

/// The hive-partition identity (`host=/user=/source=`) parsed from a key.
#[derive(Debug, PartialEq, Eq)]
struct SliceId {
    /// The `host=` segment, when present.
    host: Option<String>,
    /// The `user=` segment, when present.
    user: Option<String>,
    /// The `source=` segment.
    source: String,
}

/// Parse the hive identity from a data-file key.
///
/// Returns `None` when there is no `source=` segment: the identity is
/// incomplete, so the object is not a corpus slice.
fn parse_slice_key(key: &str) -> Option<SliceId> {
    let mut host = None;
    let mut user = None;
    let mut source = None;
    for segment in key.split('/') {
        if let Some(value) = segment.strip_prefix("host=") {
            host = Some(value.to_owned());
        } else if let Some(value) = segment.strip_prefix("user=") {
            user = Some(value.to_owned());
        } else if let Some(value) = segment.strip_prefix("source=") {
            source = Some(value.to_owned());
        }
    }
    source.map(|source| SliceId { host, user, source })
}

/// Read every `data.parquet` under the prefix as a [`Slice`], keyed by its
/// `host=/user=/source=` identity.
///
/// Unlike [`read_documents`], which flattens the whole prefix into one
/// host/user-less document set, this keeps each slice separate, so the lake fold
/// can reconcile it scoped to its origin host and user instead of silently
/// merging records across hosts.
///
/// # Errors
/// Returns an error if the client cannot be built, the prefix cannot be listed,
/// or any object cannot be read or decoded.
pub async fn read_slices(config: &Config) -> Result<Vec<Slice>> {
    let store = build_store(config)?;
    read_slices_from_store(&store, &config.prefix).await
}

/// The store-agnostic core of [`read_slices`], so tests can drive an in-memory
/// store. A `data.parquet` whose key carries no `source=` segment is skipped,
/// like the `_manifest.json` sidecar.
///
/// # Errors
/// Returns an error if the store cannot be listed, an object cannot be read, or
/// a data file cannot be parsed as the corpus parquet schema.
pub async fn read_slices_from_store(store: &dyn ObjectStore, prefix: &str) -> Result<Vec<Slice>> {
    let mut slices = Vec::new();
    let mut listing = store.list(Some(&ObjectPath::from(prefix)));
    while let Some(entry) = listing.next().await {
        let meta = entry.context(ListSnafu { prefix })?;
        let key = meta.location.to_string();
        if !key.ends_with(DATA_SUFFIX) {
            continue;
        }
        let Some(SliceId { host, user, source }) = parse_slice_key(&key) else {
            continue;
        };
        let result = store
            .get(&meta.location)
            .await
            .context(GetSnafu { key: key.clone() })?;
        let bytes = result
            .bytes()
            .await
            .context(GetSnafu { key: key.clone() })?;
        let mut documents = Vec::new();
        parse_parquet(bytes, &key, &mut documents)?;
        slices.push(Slice {
            host,
            user,
            source,
            documents,
        });
    }
    Ok(slices)
}

/// Build the S3 client. Credentials come from the environment
/// (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`).
fn build_store(config: &Config) -> Result<AmazonS3> {
    let mut builder = AmazonS3Builder::from_env()
        .with_bucket_name(&config.bucket)
        .with_region(&config.region);
    if let Some(endpoint) = &config.endpoint {
        builder = builder.with_endpoint(endpoint);
    }
    builder.build().context(BuildStoreSnafu {
        bucket: config.bucket.clone(),
    })
}

/// Parse one parquet object's record batches into documents, appending to `out`.
///
/// `object_store`'s `Bytes` is a valid [`ChunkReader`](parquet::file::reader::ChunkReader),
/// so the reader builds directly off the in-memory bytes with no temp file. Taking
/// any `ChunkReader` keeps this off a direct `bytes` dependency.
fn parse_parquet<R>(data: R, key: &str, out: &mut Vec<Document>) -> Result<()>
where
    R: parquet::file::reader::ChunkReader + 'static,
{
    let reader = ParquetRecordBatchReaderBuilder::try_new(data)
        .context(ParquetSnafu { key })?
        .build()
        .context(ParquetSnafu { key })?;
    for batch in reader {
        let batch = batch.context(DecodeSnafu { key })?;
        documents_from_batch(&batch, key, out)?;
    }
    Ok(())
}

/// Reconstruct one record batch's rows into documents.
///
/// Only the four identity/content columns are read; the rest of the sink schema
/// is a projection out of `meta_json`, so it is ignored. A missing column, a
/// mis-typed column, or a null cell in a required column is a typed error, never a
/// silent default.
fn documents_from_batch(batch: &RecordBatch, key: &str, out: &mut Vec<Document>) -> Result<()> {
    let external_id = string_column(batch, COL_EXTERNAL_ID, key)?;
    let content_hash = string_column(batch, COL_CONTENT_HASH, key)?;
    let body = string_column(batch, COL_BODY, key)?;
    let meta_json = string_column(batch, COL_META_JSON, key)?;

    out.reserve(batch.num_rows());
    for row in 0..batch.num_rows() {
        // All four columns are non-nullable in the sink schema, so a null is a
        // malformed log; `non_null_str` returns a typed `NullValue` error rather
        // than letting `value(row)` hand back an arbitrary default. The sink
        // encodes `body` via `String::from_utf8_lossy`, lossless for the UTF-8
        // corpus text every parquet-sinked source emits, so the verbatim
        // `content_hash` still describes the reconstructed bytes. `meta_json` is
        // the full metadata object as a string.
        let meta_str = non_null_str(meta_json, row, COL_META_JSON, key)?;
        let meta = serde_json::from_str(meta_str).context(MetaJsonSnafu { key })?;
        out.push(Document {
            external_id: non_null_str(external_id, row, COL_EXTERNAL_ID, key)?.to_owned(),
            file_name: non_null_str(external_id, row, COL_EXTERNAL_ID, key)?.to_owned(),
            mime: "text/plain",
            body: non_null_str(body, row, COL_BODY, key)?
                .to_owned()
                .into_bytes(),
            meta_json: meta,
            content_hash: non_null_str(content_hash, row, COL_CONTENT_HASH, key)?.to_owned(),
        });
    }
    Ok(())
}

/// Read one row of a required string column, erroring (never defaulting) on a
/// null cell. The sink writes these columns non-nullable, so a null is a
/// malformed log, not an expected absence.
fn non_null_str<'a>(
    array: &'a StringArray,
    row: usize,
    column: &'static str,
    key: &str,
) -> Result<&'a str> {
    array
        .is_valid(row)
        .then(|| array.value(row))
        .context(NullValueSnafu { key, column, row })
}

/// Borrow one column as a `StringArray`, erroring (never defaulting) when the
/// column is absent or not a `Utf8` array.
fn string_column<'a>(
    batch: &'a RecordBatch,
    column: &'static str,
    key: &str,
) -> Result<&'a StringArray> {
    let array = batch
        .column_by_name(column)
        .context(MissingColumnSnafu { key, column })?;
    array
        .as_any()
        .downcast_ref::<StringArray>()
        .context(ColumnTypeSnafu { key, column })
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "tests assert observable parse outcomes")]

    use std::sync::Arc;

    use arrow::array::{Int64Array, RecordBatch, StringArray};
    use arrow::datatypes::{DataType, Field, Schema};
    use object_store::ObjectStoreExt as _;
    use object_store::memory::InMemory;
    use object_store::path::Path as ObjectPath;
    use parquet::arrow::ArrowWriter;
    use serde_json::json;

    use super::{Error, SliceId, parse_slice_key, read_from_store, read_slices_from_store};

    /// The exact flat schema `sink-parquet` writes (kept in lockstep with its
    /// `schema()`), so this test round-trips the real on-disk shape.
    fn sink_schema() -> Schema {
        let text = |name: &str, nullable: bool| Field::new(name, DataType::Utf8, nullable);
        Schema::new(vec![
            text("external_id", false),
            text("source", false),
            text("content_hash", false),
            text("title", true),
            text("url", true),
            text("host", true),
            Field::new("timestamp", DataType::Int64, true),
            text("body", false),
            text("meta_json", false),
        ])
    }

    /// Build a 2-row parquet object in the exact sink layout, via arrow arrays +
    /// `ArrowWriter`, exactly as `sink-parquet::encode_parquet` does.
    fn encode_two_rows() -> Vec<u8> {
        let meta_a = json!({
            "source": "test", "external_id": "a", "content_hash": "sha256:aaa",
            "title": "title a", "timestamp": 100,
        });
        let meta_b = json!({
            "source": "test", "external_id": "b", "content_hash": "sha256:bbb",
            "title": "title b", "timestamp": 200,
        });
        let columns: Vec<arrow::array::ArrayRef> = vec![
            Arc::new(StringArray::from(vec!["a", "b"])),
            Arc::new(StringArray::from(vec!["test", "test"])),
            Arc::new(StringArray::from(vec!["sha256:aaa", "sha256:bbb"])),
            Arc::new(StringArray::from(vec![Some("title a"), Some("title b")])),
            Arc::new(StringArray::from(vec![None::<&str>, None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>, None::<&str>])),
            Arc::new(Int64Array::from(vec![Some(100), Some(200)])),
            Arc::new(StringArray::from(vec!["alpha", "beta"])),
            Arc::new(StringArray::from(vec![
                meta_a.to_string(),
                meta_b.to_string(),
            ])),
        ];
        let batch =
            RecordBatch::try_new(Arc::new(sink_schema()), columns).expect("build record batch");
        let mut buffer = Vec::new();
        let mut writer =
            ArrowWriter::try_new(&mut buffer, batch.schema(), None).expect("arrow writer");
        writer.write(&batch).expect("write batch");
        writer.close().expect("close writer");
        buffer
    }

    #[tokio::test]
    async fn reads_data_parquet_and_skips_manifest() {
        let store = InMemory::new();
        store
            .put(
                &ObjectPath::from("corpus/source=test/data.parquet"),
                encode_two_rows().into(),
            )
            .await
            .expect("put data");
        // A sibling manifest must be skipped, not parsed as parquet.
        store
            .put(
                &ObjectPath::from("corpus/source=test/_manifest.json"),
                serde_json::to_vec(&json!({ "content_hash": "sha256:aaa" }))
                    .expect("serialize manifest")
                    .into(),
            )
            .await
            .expect("put manifest");

        let mut docs = read_from_store(&store, "corpus").await.expect("read");
        docs.sort_by(|a, b| a.external_id.cmp(&b.external_id));
        assert_eq!(
            docs.len(),
            2,
            "exactly the two parquet rows, manifest skipped"
        );

        let a = &docs[0];
        assert_eq!(a.external_id, "a");
        assert_eq!(a.content_hash, "sha256:aaa");
        assert_eq!(a.body, b"alpha");
        assert_eq!(a.mime, "text/plain");
        // meta_json is reconstructed whole from the column, source extras intact.
        assert_eq!(a.meta_json["source"], "test");
        assert_eq!(a.meta_json["title"], "title a");
        assert_eq!(a.meta_json["timestamp"], 100);

        let b = &docs[1];
        assert_eq!(b.external_id, "b");
        assert_eq!(b.content_hash, "sha256:bbb");
        assert_eq!(b.body, b"beta");
        assert_eq!(b.meta_json["title"], "title b");
    }

    #[tokio::test]
    async fn empty_prefix_yields_no_documents() {
        let store = InMemory::new();
        let docs = read_from_store(&store, "corpus").await.expect("read");
        assert!(docs.is_empty());
    }

    #[test]
    fn parse_slice_key_extracts_hive_identity() {
        assert_eq!(
            parse_slice_key("corpus/host=h1/user=root/source=shell/data.parquet"),
            Some(SliceId {
                host: Some("h1".to_owned()),
                user: Some("root".to_owned()),
                source: "shell".to_owned()
            })
        );
        // A host-level source has no `user=` segment.
        assert_eq!(
            parse_slice_key("corpus/host=h2/source=git/data.parquet"),
            Some(SliceId {
                host: Some("h2".to_owned()),
                user: None,
                source: "git".to_owned()
            })
        );
        // No `source=` segment: not a corpus slice.
        assert_eq!(parse_slice_key("corpus/host=h2/data.parquet"), None);
    }

    #[tokio::test]
    async fn slices_preserve_per_host_identity() {
        // The lake fold relies on this: two hosts' slices must stay separate,
        // each tagged with its own host/user, never flattened together (which
        // would let a shared external_id silently clobber across hosts).
        let store = InMemory::new();
        store
            .put(
                &ObjectPath::from("corpus/host=hil-compute-1/user=root/source=shell/data.parquet"),
                encode_two_rows().into(),
            )
            .await
            .expect("put slice a");
        store
            .put(
                &ObjectPath::from("corpus/host=hil-compute-2/source=git/data.parquet"),
                encode_two_rows().into(),
            )
            .await
            .expect("put slice b");
        // A manifest sibling must be skipped, like the flat read.
        store
            .put(
                &ObjectPath::from(
                    "corpus/host=hil-compute-1/user=root/source=shell/_manifest.json",
                ),
                serde_json::to_vec(&json!({ "content_hash": "sha256:aaa" }))
                    .expect("serialize manifest")
                    .into(),
            )
            .await
            .expect("put manifest");

        let mut slices = read_slices_from_store(&store, "corpus")
            .await
            .expect("read slices");
        slices.sort_by(|a, b| a.source.cmp(&b.source));
        assert_eq!(
            slices.len(),
            2,
            "one slice per data.parquet, manifest skipped"
        );

        let git = &slices[0];
        assert_eq!(git.source, "git");
        assert_eq!(git.host.as_deref(), Some("hil-compute-2"));
        assert_eq!(git.user, None, "a host-level source carries no user");
        assert_eq!(git.documents.len(), 2);

        let shell = &slices[1];
        assert_eq!(shell.source, "shell");
        assert_eq!(shell.host.as_deref(), Some("hil-compute-1"));
        assert_eq!(shell.user.as_deref(), Some("root"));
        assert_eq!(shell.documents.len(), 2);
    }

    /// A schema whose required columns are written nullable, so a row can carry a
    /// null in a column the sink would write non-nullable. Used to prove the
    /// `NullValue` guard fires instead of `value(row)` defaulting.
    fn nullable_schema() -> Schema {
        let text = |name: &str| Field::new(name, DataType::Utf8, true);
        Schema::new(vec![
            text("external_id"),
            text("source"),
            text("content_hash"),
            text("title"),
            text("url"),
            text("host"),
            Field::new("timestamp", DataType::Int64, true),
            text("body"),
            text("meta_json"),
        ])
    }

    /// One row whose `content_hash` is null. A malformed log: every required
    /// column is non-nullable in the real sink schema.
    fn encode_null_content_hash() -> Vec<u8> {
        let meta = json!({ "source": "test", "external_id": "a" });
        let columns: Vec<arrow::array::ArrayRef> = vec![
            Arc::new(StringArray::from(vec![Some("a")])),
            Arc::new(StringArray::from(vec![Some("test")])),
            // The null cell under test.
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(Int64Array::from(vec![None::<i64>])),
            Arc::new(StringArray::from(vec![Some("alpha")])),
            Arc::new(StringArray::from(vec![Some(meta.to_string())])),
        ];
        let batch =
            RecordBatch::try_new(Arc::new(nullable_schema()), columns).expect("build record batch");
        let mut buffer = Vec::new();
        let mut writer =
            ArrowWriter::try_new(&mut buffer, batch.schema(), None).expect("arrow writer");
        writer.write(&batch).expect("write batch");
        writer.close().expect("close writer");
        buffer
    }

    #[tokio::test]
    async fn null_in_required_column_is_a_typed_error() {
        let store = InMemory::new();
        store
            .put(
                &ObjectPath::from("corpus/source=test/data.parquet"),
                encode_null_content_hash().into(),
            )
            .await
            .expect("put data");

        let error = read_from_store(&store, "corpus")
            .await
            .expect_err("a null must error");
        assert!(
            matches!(
                error,
                Error::NullValue {
                    column: "content_hash",
                    row: 0,
                    ..
                }
            ),
            "a null required column must yield a typed NullValue error, got {error:?}"
        );
    }
}
