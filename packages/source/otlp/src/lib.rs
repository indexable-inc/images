//! Read the OTLP/JSON log objects an `OpenTelemetry` Collector wrote to S3 back
//! into search [`Document`]s.
//!
//! This is the consumer half of the ingestion bus (RFC 0004). The `sink-otlp`
//! emitter sends each corpus record to the collector as an OTLP log record; the
//! collector's `awss3` exporter (configured `marshaler = otlp_json`,
//! `compression = none`) writes those records as OTLP `ExportLogsServiceRequest`
//! JSON objects under a bucket prefix. This crate lists that prefix, parses each
//! object, and reconstructs the [`Document`]s so the indexer can reconcile them
//! into Mixedbread. It pairs with `sink-otlp`: the attribute names it reads back
//! are the ones that sink writes.
//!
//! A log record is only turned into a document when it carries the `external_id`
//! attribute a corpus record always has; any other log on the bus (a stray app
//! log) is skipped rather than mis-ingested. The document's `content_hash` is
//! recomputed from the reconstructed body, not read from an attribute, so it
//! always describes the bytes that get embedded.
//!
//! Known limitation: this lists the whole prefix and materializes every record
//! each run, and the emitter is append-only, so the archive and the in-memory
//! set grow with runs. Before this carries real volume it needs an incremental
//! cursor (only read partitions newer than the last consumed) plus a retention
//! policy on the bucket. Tracked as follow-up; fine for the initial wiring.

#![forbid(unsafe_code)]

use futures::stream::StreamExt as _;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use object_store::{ObjectStore, ObjectStoreExt as _};
use serde::Deserialize;
use source_meta::{Document, keys};
use snafu::{ResultExt as _, Snafu};

/// The attribute name `sink-otlp` writes for the document's `external_id` (a
/// `Document` field, surfaced as a metadata key on the bus). Read back here to
/// re-key the reconstructed document.
const EXTERNAL_ID: &str = "external_id";

/// Connection and layout for reading the collector's S3 archive.
#[derive(Debug, Clone)]
pub struct Config {
    /// Bucket the collector's `awss3` exporter writes to.
    pub bucket: String,
    /// S3 endpoint URL. `None` uses AWS S3; for a self-hosted store pass its endpoint.
    pub endpoint: Option<String>,
    /// Region (`auto` for non-AWS stores).
    pub region: String,
    /// Key prefix under the bucket to read OTLP objects from.
    pub prefix: String,
}

/// Failures from the OTLP reader.
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
    #[snafu(display("failed to list OTLP objects under {prefix}"))]
    List {
        /// Prefix being listed.
        prefix: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// An object could not be read.
    #[snafu(display("failed to read OTLP object {key}"))]
    Get {
        /// Object key.
        key: String,
        /// Underlying object-store error.
        source: object_store::Error,
    },
    /// An object did not parse as an OTLP/JSON `ExportLogsServiceRequest`.
    #[snafu(display("failed to parse OTLP object {key}"))]
    Parse {
        /// Object key.
        key: String,
        /// Underlying serde error.
        source: serde_json::Error,
    },
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Read every OTLP object under the configured prefix and reconstruct the corpus
/// documents they carry.
///
/// # Errors
/// Returns an error if the client cannot be built, the prefix cannot be listed,
/// or any object cannot be read or parsed.
pub async fn read_documents(config: &Config) -> Result<Vec<Document>> {
    let store = build_store(config)?;
    read_from_store(&store, &config.prefix).await
}

/// The store-agnostic core of [`read_documents`], so tests can drive an
/// in-memory store.
async fn read_from_store(store: &dyn ObjectStore, prefix: &str) -> Result<Vec<Document>> {
    let mut documents = Vec::new();
    let mut listing = store.list(Some(&ObjectPath::from(prefix)));
    while let Some(entry) = listing.next().await {
        let meta = entry.context(ListSnafu { prefix })?;
        let key = meta.location.to_string();
        let result = store.get(&meta.location).await.context(GetSnafu { key: key.clone() })?;
        let bytes = result.bytes().await.context(GetSnafu { key: key.clone() })?;
        let parsed = parse_export(&bytes).context(ParseSnafu { key })?;
        documents.extend(parsed);
    }
    Ok(documents)
}

/// Build the S3 client. Credentials come from the environment
/// (`AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY`).
fn build_store(config: &Config) -> Result<AmazonS3> {
    let mut builder =
        AmazonS3Builder::from_env().with_bucket_name(&config.bucket).with_region(&config.region);
    if let Some(endpoint) = &config.endpoint {
        builder = builder.with_endpoint(endpoint);
    }
    builder.build().context(BuildStoreSnafu { bucket: config.bucket.clone() })
}

/// Parse one object's OTLP/JSON `ExportLogsServiceRequest` into documents,
/// skipping any record that is not a corpus record.
fn parse_export(bytes: &[u8]) -> std::result::Result<Vec<Document>, serde_json::Error> {
    let request: ExportLogsServiceRequest = serde_json::from_slice(bytes)?;
    let documents = request
        .resource_logs
        .into_iter()
        .flat_map(|resource| resource.scope_logs)
        .flat_map(|scope| scope.log_records)
        .filter_map(document_from_record)
        .collect();
    Ok(documents)
}

/// Reconstruct one [`Document`] from a log record, or `None` if it lacks the
/// `external_id`/`content_hash` a corpus record always carries.
///
/// Every text corpus record is `text/plain` (the source adapters all emit that),
/// and the file name is cosmetic for Mixedbread (records are addressed by
/// `external_id`), so both are set rather than carried over the bus.
fn document_from_record(record: LogRecord) -> Option<Document> {
    let mut meta = serde_json::Map::new();
    for attribute in record.attributes {
        meta.insert(attribute.key, attribute.value.into_json());
    }
    // `external_id` is the corpus identity; a record without it is a stray log.
    let external_id = meta.get(EXTERNAL_ID)?.as_str()?.to_owned();
    let body = record.body.string_value().into_bytes();
    // Recompute the hash from the reconstructed body so it always describes the
    // bytes that get embedded (source_meta invariant #1), rather than trusting a
    // possibly-stale `content_hash` attribute. Keep meta_json consistent with it.
    let content_hash = source_meta::hash_body(&body);
    meta.insert(keys::CONTENT_HASH.to_owned(), serde_json::Value::String(content_hash.clone()));
    Some(Document {
        file_name: external_id.clone(),
        external_id,
        mime: "text/plain",
        body,
        meta_json: serde_json::Value::Object(meta),
        content_hash,
    })
}

// --- OTLP/HTTP JSON model (the subset this reader consumes) ---

/// Top-level OTLP logs payload.
#[derive(Debug, Deserialize)]
struct ExportLogsServiceRequest {
    #[serde(rename = "resourceLogs", default)]
    resource_logs: Vec<ResourceLogs>,
}

/// Records sharing one resource.
#[derive(Debug, Deserialize)]
struct ResourceLogs {
    #[serde(rename = "scopeLogs", default)]
    scope_logs: Vec<ScopeLogs>,
}

/// Records sharing one instrumentation scope.
#[derive(Debug, Deserialize)]
struct ScopeLogs {
    #[serde(rename = "logRecords", default)]
    log_records: Vec<LogRecord>,
}

/// One OTLP log record.
#[derive(Debug, Deserialize)]
struct LogRecord {
    #[serde(default)]
    body: AnyValue,
    #[serde(default)]
    attributes: Vec<KeyValue>,
}

/// One OTLP attribute.
#[derive(Debug, Deserialize)]
struct KeyValue {
    key: String,
    #[serde(default)]
    value: AnyValue,
}

/// The OTLP `AnyValue` shapes this reader understands; unknown shapes deserialize
/// to the empty default and reconstruct as an empty string.
#[derive(Debug, Default, Deserialize)]
struct AnyValue {
    #[serde(rename = "stringValue")]
    string: Option<String>,
    #[serde(rename = "intValue")]
    int: Option<String>,
    #[serde(rename = "boolValue")]
    boolean: Option<bool>,
}

impl AnyValue {
    /// The string body of a record (empty when the value was not a string).
    fn string_value(self) -> String {
        self.string.unwrap_or_default()
    }

    /// Map back to a JSON value: OTLP encodes int64 as a string, so an `intValue`
    /// becomes a JSON number when it parses and a string otherwise; a `boolValue`
    /// becomes a bool; anything else becomes its string form.
    fn into_json(self) -> serde_json::Value {
        if let Some(string) = self.string {
            return serde_json::Value::String(string);
        }
        if let Some(boolean) = self.boolean {
            return serde_json::Value::Bool(boolean);
        }
        if let Some(int) = self.int {
            return int.parse::<i64>().map_or(serde_json::Value::String(int), serde_json::Value::from);
        }
        serde_json::Value::String(String::new())
    }
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "tests assert observable parse outcomes")]

    use object_store::ObjectStoreExt as _;
    use object_store::memory::InMemory;
    use object_store::path::Path as ObjectPath;
    use serde_json::json;

    use super::read_from_store;

    /// One OTLP/JSON object in the exact shape `sink-otlp` emits.
    fn otlp_object() -> Vec<u8> {
        let request = json!({
            "resourceLogs": [{
                "resource": { "attributes": [{ "key": "source", "value": { "stringValue": "shell" } }] },
                "scopeLogs": [{
                    "scope": { "name": "sink-otlp" },
                    "logRecords": [
                        {
                            "timeUnixNano": "100000000000",
                            "body": { "stringValue": "ls -la" },
                            "attributes": [
                                { "key": "source", "value": { "stringValue": "shell" } },
                                { "key": "external_id", "value": { "stringValue": "atuin:1" } },
                                { "key": "content_hash", "value": { "stringValue": "abc123" } },
                                { "key": "exit_status", "value": { "intValue": "0" } }
                            ]
                        },
                        {
                            "body": { "stringValue": "a stray app log with no corpus identity" },
                            "attributes": [{ "key": "level", "value": { "stringValue": "info" } }]
                        }
                    ]
                }]
            }]
        });
        serde_json::to_vec(&request).expect("serialize")
    }

    #[tokio::test]
    async fn reconstructs_corpus_records_and_skips_others() {
        let store = InMemory::new();
        store
            .put(&ObjectPath::from("corpus/year=2026/data.json"), otlp_object().into())
            .await
            .expect("put");

        let docs = read_from_store(&store, "corpus").await.expect("read");
        // The stray non-corpus log is skipped; only the real record comes back.
        assert_eq!(docs.len(), 1);
        let doc = &docs[0];
        assert_eq!(doc.external_id, "atuin:1");
        // content_hash is recomputed from the reconstructed body, not the stale
        // "abc123" attribute, so it describes the bytes that get embedded.
        assert_eq!(doc.content_hash, source_meta::hash_body(b"ls -la"));
        assert_eq!(doc.body, b"ls -la");
        assert_eq!(doc.mime, "text/plain");
        // String attribute round-trips as a string; int64 attribute as a number.
        assert_eq!(doc.meta_json["source"], "shell");
        assert_eq!(doc.meta_json["exit_status"], 0);
    }

    #[tokio::test]
    async fn empty_prefix_yields_no_documents() {
        let store = InMemory::new();
        let docs = read_from_store(&store, "corpus").await.expect("read");
        assert!(docs.is_empty());
    }
}
