//! Generic S3/R2 parquet sink for the multi-source search corpus.
//!
//! Every source's [`Document`]s share one flat schema (one row per document),
//! so the whole corpus is queryable with one polars/duckdb call regardless of
//! source. Per-source extras live in the `meta_json` column rather than as typed
//! columns, keeping the schema uniform.
//!
//! # Layout
//! One parquet file per source at `<prefix>/source=<source>/data.parquet`,
//! rewritten in full each run. A sibling `<prefix>/source=<source>/_manifest.json`
//! records a content hash over the source's `(external_id, content_hash)` set, so
//! a run whose corpus is unchanged skips the rewrite entirely.
//!
//! This trades incremental writes for idempotence and zero dedup-on-read: a
//! source's file always reflects its current desired state, with no accumulating
//! per-record objects. A very large source (a full git history) rewrites its
//! whole file each change; sharding is a future refinement, not a correctness
//! issue.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::sync::Arc;

use arrow::array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use parquet::arrow::ArrowWriter;
use sha2::{Digest as _, Sha256};
use snafu::{IntoError as _, ResultExt as _, Snafu};
use source_meta::{Document, Reconciler, Source, keys};

/// Connection and layout for the S3/R2 parquet sink.
#[derive(Debug, Clone)]
pub struct Config {
    /// Target bucket name.
    pub bucket: String,
    /// S3 endpoint URL. `None` uses AWS S3; for R2 pass the account endpoint.
    pub endpoint: Option<String>,
    /// Region (`auto` for R2).
    pub region: String,
    /// Key prefix under the bucket (e.g. `corpus`).
    pub prefix: String,
}

/// Failures from the parquet sink.
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
    /// An object could not be read.
    #[snafu(display("failed to read object {path}"))]
    Get {
        /// Object key.
        path: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// An object could not be written.
    #[snafu(display("failed to write object {path}"))]
    Put {
        /// Object key.
        path: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// The manifest object did not parse as JSON.
    #[snafu(display("failed to parse the manifest {path}"))]
    Manifest {
        /// Manifest key.
        path: String,
        /// Underlying serde error.
        source: serde_json::Error,
    },
    /// The manifest could not be serialized.
    #[snafu(display("failed to serialize the manifest"))]
    SerializeManifest {
        /// Underlying serde error.
        source: serde_json::Error,
    },
    /// A record batch could not be assembled.
    #[snafu(display("failed to build the record batch"))]
    Batch {
        /// Underlying arrow error.
        source: arrow::error::ArrowError,
    },
    /// Parquet encoding failed.
    #[snafu(display("failed to encode parquet"))]
    Encode {
        /// Underlying parquet error.
        source: parquet::errors::ParquetError,
    },
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Outcome of one sink pass for a source.
#[derive(Debug, Clone, Copy)]
pub struct Report {
    /// Rows written, or 0 when the pass was skipped or the source was empty.
    pub rows: usize,
    /// Whether the write was skipped because the corpus hash was unchanged.
    pub skipped: bool,
}

/// Reconciles a source's documents into the bucket as one parquet file per
/// source. The production store is [`AmazonS3`] (see [`Config::connect`]);
/// tests drive an in-memory store.
#[derive(Debug, Clone)]
pub struct ParquetReconciler<S = AmazonS3> {
    /// The object store written to.
    pub store: S,
    /// Key prefix every object lands under (e.g. `corpus/host=<host>`).
    pub prefix: String,
}

impl Config {
    /// Build the S3-backed reconciler for this config. Credentials come from
    /// the environment (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`).
    ///
    /// # Errors
    /// Returns an error if the S3 client cannot be built from the config and
    /// environment.
    pub fn connect(&self) -> Result<ParquetReconciler> {
        Ok(ParquetReconciler {
            store: build_store(self)?,
            prefix: self.prefix.clone(),
        })
    }
}

impl<S: Clone> ParquetReconciler<S> {
    /// The same store, writing under a different prefix (the indexer derives
    /// per-user prefixes from one connected reconciler this way).
    #[must_use]
    pub fn with_prefix(&self, prefix: impl Into<String>) -> Self {
        Self {
            store: self.store.clone(),
            prefix: prefix.into(),
        }
    }
}

impl<S: ObjectStore> Reconciler for ParquetReconciler<S> {
    type Report = Report;
    type Error = Error;

    /// Write `documents` as `<prefix>/source=<source>/data.parquet` in one
    /// full-file overwrite, skipping the rewrite when the corpus hash in the
    /// sibling manifest is unchanged. The file always reflects the source's
    /// current desired state, so a document absent from `documents` simply
    /// vanishes with the rewrite.
    async fn reconcile(&self, source: &Source, documents: &[Document]) -> Result<Report> {
        if documents.is_empty() {
            return Ok(Report {
                rows: 0,
                skipped: true,
            });
        }

        let prefix = &self.prefix;
        let data_path = ObjectPath::from(format!("{prefix}/source={source}/data.parquet"));
        let manifest_path = ObjectPath::from(format!("{prefix}/source={source}/_manifest.json"));
        let hash = corpus_hash(documents);
        if load_manifest(&self.store, &manifest_path).await? == Some(hash.clone()) {
            return Ok(Report {
                rows: 0,
                skipped: true,
            });
        }

        let batch = record_batch(documents)?;
        let bytes = encode_parquet(&batch)?;
        self.store
            .put(&data_path, PutPayload::from(bytes))
            .await
            .context(PutSnafu {
                path: data_path.to_string(),
            })?;
        save_manifest(&self.store, &manifest_path, &hash).await?;
        Ok(Report {
            rows: documents.len(),
            skipped: false,
        })
    }
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

/// The flat corpus schema: identity, the common header fields, the embedded
/// body, and the full metadata object as JSON for source-specific extras.
fn schema() -> Schema {
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

/// Build the record batch: one row per document, projecting the common header
/// fields out of each document's flat metadata.
fn record_batch(documents: &[Document]) -> Result<RecordBatch> {
    let meta_str = |doc: &Document, key: &str| {
        doc.meta_json
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let columns: Vec<ArrayRef> = vec![
        Arc::new(
            documents
                .iter()
                .map(|d| Some(d.external_id.as_str()))
                .collect::<StringArray>(),
        ),
        Arc::new(
            documents
                .iter()
                .map(|d| meta_str(d, keys::SOURCE))
                .collect::<StringArray>(),
        ),
        Arc::new(
            documents
                .iter()
                .map(|d| Some(d.content_hash.as_str()))
                .collect::<StringArray>(),
        ),
        Arc::new(
            documents
                .iter()
                .map(|d| meta_str(d, keys::TITLE))
                .collect::<StringArray>(),
        ),
        Arc::new(
            documents
                .iter()
                .map(|d| meta_str(d, "url"))
                .collect::<StringArray>(),
        ),
        Arc::new(
            documents
                .iter()
                .map(|d| meta_str(d, keys::HOST))
                .collect::<StringArray>(),
        ),
        Arc::new(
            documents
                .iter()
                .map(|d| {
                    d.meta_json
                        .get(keys::TIMESTAMP)
                        .and_then(serde_json::Value::as_i64)
                })
                .collect::<Int64Array>(),
        ),
        Arc::new(
            documents
                .iter()
                .map(|d| Some(String::from_utf8_lossy(&d.body).into_owned()))
                .collect::<StringArray>(),
        ),
        Arc::new(
            documents
                .iter()
                .map(|d| Some(d.meta_json.to_string()))
                .collect::<StringArray>(),
        ),
    ];
    RecordBatch::try_new(Arc::new(schema()), columns).context(BatchSnafu)
}

/// Encode a record batch to parquet bytes in memory.
fn encode_parquet(batch: &RecordBatch) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut writer =
        ArrowWriter::try_new(&mut buffer, batch.schema(), None).context(EncodeSnafu)?;
    writer.write(batch).context(EncodeSnafu)?;
    writer.close().context(EncodeSnafu)?;
    Ok(buffer)
}

/// Content hash of a source's corpus: sha256 over each document's external id
/// and content hash, in a stable sorted order. Adding, removing, or changing any
/// record changes the hash and triggers a rewrite; an unchanged corpus matches
/// and is skipped.
fn corpus_hash(documents: &[Document]) -> String {
    let mut pairs: BTreeSet<(&str, &str)> = BTreeSet::new();
    for document in documents {
        pairs.insert((
            document.external_id.as_str(),
            document.content_hash.as_str(),
        ));
    }
    let mut digest = Sha256::new();
    for (external_id, content_hash) in pairs {
        digest.update(external_id.as_bytes());
        digest.update([0]);
        digest.update(content_hash.as_bytes());
        digest.update([0]);
    }
    hex::encode(digest.finalize())
}

/// Load the per-source manifest's corpus hash, or `None` when it does not exist
/// (or lacks the field, which safely forces a rewrite).
async fn load_manifest(store: &dyn ObjectStore, path: &ObjectPath) -> Result<Option<String>> {
    let result = match store.get(path).await {
        Ok(result) => result,
        Err(object_store::Error::NotFound { .. }) => return Ok(None),
        Err(source) => {
            return Err(GetSnafu {
                path: path.to_string(),
            }
            .into_error(source));
        }
    };
    let bytes = result.bytes().await.context(GetSnafu {
        path: path.to_string(),
    })?;
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).context(ManifestSnafu {
        path: path.to_string(),
    })?;
    Ok(manifest
        .get("content_hash")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned))
}

/// Write the per-source manifest with the current corpus hash.
async fn save_manifest(store: &dyn ObjectStore, path: &ObjectPath, hash: &str) -> Result<()> {
    let bytes = serde_json::to_vec(&serde_json::json!({ "content_hash": hash }))
        .context(SerializeManifestSnafu)?;
    store
        .put(path, PutPayload::from(bytes))
        .await
        .context(PutSnafu {
            path: path.to_string(),
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ParquetReconciler, Report};
    use object_store::ObjectStoreExt;
    use object_store::memory::InMemory;
    use object_store::path::Path as ObjectPath;
    use serde_json::json;
    use source_meta::{Document, Reconciler as _, Source};

    fn doc(id: &str, body: &str) -> Document {
        let content_hash = source_meta::hash_body(body.as_bytes());
        Document {
            external_id: id.to_owned(),
            file_name: format!("{id}.txt"),
            mime: "text/plain",
            body: body.as_bytes().to_vec(),
            meta_json: json!({
                "source": "test",
                "external_id": id,
                "content_hash": content_hash,
                "title": format!("title {id}"),
                "timestamp": 100,
            }),
            content_hash,
        }
    }

    #[tokio::test]
    async fn writes_parquet_and_manifest_then_skips_unchanged() {
        let reconciler = ParquetReconciler {
            store: InMemory::new(),
            prefix: "corpus".to_owned(),
        };
        let source = Source::new("test");
        let docs = vec![doc("a", "alpha"), doc("b", "beta")];

        let first: Report = reconciler
            .reconcile(&source, &docs)
            .await
            .expect("first sync");
        assert_eq!(first.rows, 2);
        assert!(!first.skipped);

        // The parquet file and manifest both landed under the source partition.
        let data = ObjectPath::from("corpus/source=test/data.parquet");
        let manifest = ObjectPath::from("corpus/source=test/_manifest.json");
        assert!(reconciler.store.get(&data).await.is_ok());
        assert!(reconciler.store.get(&manifest).await.is_ok());

        // A second identical run is a no-op (corpus hash unchanged).
        let second = reconciler
            .reconcile(&source, &docs)
            .await
            .expect("second sync");
        assert!(second.skipped);
        assert_eq!(second.rows, 0);
    }

    #[tokio::test]
    async fn empty_source_writes_nothing() {
        let reconciler = ParquetReconciler {
            store: InMemory::new(),
            prefix: "corpus".to_owned(),
        };
        let report = reconciler
            .reconcile(&Source::new("test"), &[])
            .await
            .expect("sync");
        assert!(report.skipped);
        assert_eq!(report.rows, 0);
    }

    #[tokio::test]
    async fn with_prefix_scopes_writes_under_the_new_prefix() {
        // `with_prefix` derives the per-user reconciler in the fleet path; the
        // derived writer must land objects under the new prefix, not the base.
        let store = std::sync::Arc::new(InMemory::new());
        let base = ParquetReconciler {
            store: std::sync::Arc::clone(&store),
            prefix: "corpus/host=h".to_owned(),
        };
        let scoped = base.with_prefix("corpus/host=h/user=alice");

        scoped
            .reconcile(&Source::new("test"), &[doc("a", "alpha")])
            .await
            .expect("sync");
        let scoped_path = ObjectPath::from("corpus/host=h/user=alice/source=test/data.parquet");
        let base_path = ObjectPath::from("corpus/host=h/source=test/data.parquet");
        assert!(store.get(&scoped_path).await.is_ok());
        assert!(
            store.get(&base_path).await.is_err(),
            "base prefix must stay untouched"
        );
    }
}
