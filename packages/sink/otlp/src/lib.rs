//! OTLP/HTTP logs sink for the multi-source search corpus.
//!
//! Each source's [`Document`]s are emitted to an `OpenTelemetry` Collector as
//! OTLP log records (`POST {endpoint}/v1/logs`, JSON encoding). This is the
//! emitter half of the ingestion bus (RFC 0004): the collector receives one
//! record per document and fans it out to its own exporters (`ClickHouse` for
//! Grafana, S3 for the durable archive), so a new corpus consumer is a new
//! collector exporter rather than a new sink compiled into every producer.
//!
//! The document's flat metadata is projected onto the log record so no
//! information is lost crossing the bus: the body becomes the record body, the
//! source becomes a resource attribute, and every metadata key (including
//! `external_id` and `content_hash`, which a downstream consumer needs to
//! reconstruct the document and skip-if-unchanged) becomes a record attribute.
//!
//! Emission is append-only: every run sends the current record set and the
//! collector's downstream consumers dedup by `content_hash`. The sink does not
//! itself skip unchanged corpora, because unlike a single rewritten S3 object
//! there is no per-source manifest to compare against on the bus.

#![forbid(unsafe_code)]

use serde::Serialize;
use source_meta::{Document, SourceAdapter, keys};
use snafu::{IntoError as _, ResultExt as _, Snafu};

/// Nanoseconds per second, for converting an epoch-second timestamp to the
/// `timeUnixNano` OTLP field.
const NANOS_PER_SEC: i64 = 1_000_000_000;

/// Records per OTLP request. Large sources (a full shell history) are chunked so
/// no single request grows unbounded; the collector batches internally anyway.
const RECORDS_PER_REQUEST: usize = 500;

/// Connection settings for the OTLP/HTTP logs sink.
#[derive(Debug, Clone)]
pub struct Config {
    /// Collector OTLP/HTTP base URL, e.g. `http://127.0.0.1:4318`. Records are
    /// posted to `<endpoint>/v1/logs`.
    pub endpoint: String,
}

/// Failures from the OTLP sink.
#[derive(Debug, Snafu)]
#[non_exhaustive]
pub enum Error {
    /// The source adapter failed while producing documents.
    #[snafu(display("source adapter failed while producing documents"))]
    Adapter {
        /// Underlying adapter error.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The HTTP request to the collector could not be sent.
    #[snafu(display("failed to POST OTLP logs to {url}"))]
    Request {
        /// Target URL.
        url: String,
        /// Underlying reqwest error.
        source: reqwest::Error,
    },
    /// The collector rejected the batch with a non-success status.
    #[snafu(display("collector returned {status} for {url}: {body}"))]
    Http {
        /// Target URL.
        url: String,
        /// HTTP status code.
        status: u16,
        /// Response body, for the failure reason.
        body: String,
    },
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Outcome of one sink pass for a source.
#[derive(Debug, Clone, Copy)]
pub struct Report {
    /// Log records emitted to the collector.
    pub records: usize,
    /// Whether the pass was skipped because the source produced no documents.
    pub skipped: bool,
}

/// Emit one source's documents to the collector as OTLP log records.
///
/// # Errors
/// Returns an error if the adapter fails, a request cannot be sent, or the
/// collector rejects a batch.
pub async fn sync<A: SourceAdapter + Sync>(adapter: &A, config: &Config) -> Result<Report> {
    let source = adapter.source();
    let mut documents = Vec::new();
    for document in adapter.documents() {
        documents.push(document.map_err(|err| AdapterSnafu.into_error(Box::new(err)))?);
    }
    if documents.is_empty() {
        return Ok(Report { records: 0, skipped: true });
    }

    let url = format!("{}/v1/logs", config.endpoint.trim_end_matches('/'));
    let client = reqwest::Client::new();
    let total = documents.len();
    for chunk in documents.chunks(RECORDS_PER_REQUEST) {
        let request = export_request(source.as_str(), chunk);
        post(&client, &url, &request).await?;
    }
    Ok(Report { records: total, skipped: false })
}

/// POST one OTLP `ExportLogsServiceRequest` and turn a non-success status into a
/// typed [`Error::Http`] carrying the response body.
async fn post(client: &reqwest::Client, url: &str, request: &ExportLogsServiceRequest) -> Result<()> {
    let response = client.post(url).json(request).send().await.context(RequestSnafu { url })?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.unwrap_or_else(|_| "<unreadable body>".to_owned());
    Err(HttpSnafu { url, status: status.as_u16(), body }.build())
}

/// Build the OTLP request for one source's chunk of documents: a single
/// `resourceLogs` whose resource names the producer and source, and one log
/// record per document.
fn export_request(source: &str, documents: &[Document]) -> ExportLogsServiceRequest {
    let resource = Resource {
        attributes: vec![
            KeyValue::string("service.name", "indexer"),
            KeyValue::string(keys::SOURCE, source),
        ],
    };
    let log_records = documents.iter().map(log_record).collect();
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource,
            scope_logs: vec![ScopeLogs { scope: Scope { name: "sink-otlp" }, log_records }],
        }],
    }
}

/// Project one document onto an OTLP log record: the body becomes the record
/// body, and every flat metadata key becomes a record attribute so the document
/// can be reconstructed downstream.
fn log_record(document: &Document) -> LogRecord {
    let nanos = document
        .meta_json
        .get(keys::TIMESTAMP)
        .and_then(serde_json::Value::as_i64)
        .map_or(0, |secs| secs.saturating_mul(NANOS_PER_SEC));
    let attributes = match &document.meta_json {
        serde_json::Value::Object(map) => {
            map.iter().map(|(key, value)| KeyValue::from_json(key, value)).collect()
        }
        _ => Vec::new(),
    };
    LogRecord {
        time_unix_nano: nanos.to_string(),
        observed_time_unix_nano: nanos.to_string(),
        body: AnyValue::String(String::from_utf8_lossy(&document.body).into_owned()),
        attributes,
    }
}

// --- OTLP/HTTP JSON model (the subset this sink emits) ---

/// Top-level OTLP logs payload.
#[derive(Debug, Serialize)]
struct ExportLogsServiceRequest {
    #[serde(rename = "resourceLogs")]
    resource_logs: Vec<ResourceLogs>,
}

/// Records sharing one resource (here, one source).
#[derive(Debug, Serialize)]
struct ResourceLogs {
    resource: Resource,
    #[serde(rename = "scopeLogs")]
    scope_logs: Vec<ScopeLogs>,
}

/// The resource attributes shared by every record in a batch.
#[derive(Debug, Serialize)]
struct Resource {
    attributes: Vec<KeyValue>,
}

/// Records sharing one instrumentation scope.
#[derive(Debug, Serialize)]
struct ScopeLogs {
    scope: Scope,
    #[serde(rename = "logRecords")]
    log_records: Vec<LogRecord>,
}

/// Names the emitter.
#[derive(Debug, Serialize)]
struct Scope {
    name: &'static str,
}

/// One OTLP log record.
#[derive(Debug, Serialize)]
struct LogRecord {
    #[serde(rename = "timeUnixNano")]
    time_unix_nano: String,
    #[serde(rename = "observedTimeUnixNano")]
    observed_time_unix_nano: String,
    body: AnyValue,
    attributes: Vec<KeyValue>,
}

/// An OTLP attribute (key plus typed value).
#[derive(Debug, Serialize)]
struct KeyValue {
    key: String,
    value: AnyValue,
}

impl KeyValue {
    /// A string-valued attribute.
    fn string(key: &str, value: &str) -> Self {
        Self { key: key.to_owned(), value: AnyValue::String(value.to_owned()) }
    }

    /// Map a JSON metadata value to the closest OTLP attribute value: strings and
    /// booleans map directly, integers use OTLP's string-encoded int64, and any
    /// other shape (float, array, object, null) is carried as its JSON text so no
    /// metadata is dropped crossing the bus.
    fn from_json(key: &str, value: &serde_json::Value) -> Self {
        let any = match value {
            serde_json::Value::String(string) => AnyValue::String(string.clone()),
            serde_json::Value::Bool(boolean) => AnyValue::Bool(*boolean),
            serde_json::Value::Number(number) => number
                .as_i64()
                .map_or_else(|| AnyValue::String(number.to_string()), |int| AnyValue::Int(int.to_string())),
            other => AnyValue::String(other.to_string()),
        };
        Self { key: key.to_owned(), value: any }
    }
}

/// The OTLP `AnyValue` shapes this sink emits. Externally tagged so each
/// serializes to OTLP's `{"stringValue": ...}` / `{"intValue": ...}` form.
#[derive(Debug, Serialize)]
enum AnyValue {
    #[serde(rename = "stringValue")]
    String(String),
    #[serde(rename = "intValue")]
    Int(String),
    #[serde(rename = "boolValue")]
    Bool(bool),
}

#[cfg(test)]
mod tests {
    #![expect(clippy::expect_used, reason = "tests assert observable request outcomes")]

    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::Mutex;

    use axum::Router;
    use axum::extract::State;
    use axum::routing::post;
    use serde_json::json;
    use source_meta::{Document, Source, SourceAdapter};

    use super::{Config, sync};

    /// A throwaway receiver: its `endpoint` plus the bodies it captured.
    struct Receiver {
        endpoint: String,
        captured: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    struct TestSource {
        docs: Vec<Document>,
    }

    impl SourceAdapter for TestSource {
        type Error = std::convert::Infallible;
        fn source(&self) -> Source {
            Source::new("shell")
        }
        fn documents(&self) -> impl Iterator<Item = Result<Document, Self::Error>> + Send {
            self.docs.clone().into_iter().map(Ok)
        }
    }

    fn doc(id: &str, body: &str) -> Document {
        let content_hash = source_meta::hash_body(body.as_bytes());
        Document {
            external_id: id.to_owned(),
            file_name: format!("{id}.txt"),
            mime: "text/plain",
            body: body.as_bytes().to_vec(),
            meta_json: json!({
                "source": "shell",
                "external_id": id,
                "content_hash": content_hash,
                "timestamp": 100,
                "exit_status": 0,
            }),
            content_hash,
        }
    }

    /// Stand up a throwaway OTLP/HTTP receiver, capturing every posted body.
    async fn spawn_receiver() -> Receiver {
        let captured: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/v1/logs",
                post(|State(state): State<Arc<Mutex<Vec<serde_json::Value>>>>, body: String| async move {
                    let value: serde_json::Value = serde_json::from_str(&body).expect("valid json body");
                    state.lock().expect("lock").push(value);
                    "{}"
                }),
            )
            .with_state(Arc::clone(&captured));
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Receiver { endpoint: format!("http://{addr}"), captured }
    }

    #[tokio::test]
    async fn emits_one_record_per_document() {
        let receiver = spawn_receiver().await;
        let adapter = TestSource { docs: vec![doc("a", "ls"), doc("b", "cd /tmp")] };

        let report = sync(&adapter, &Config { endpoint: receiver.endpoint }).await.expect("sync");
        assert_eq!(report.records, 2);
        assert!(!report.skipped);

        // Clone out so the mutex guard drops here, not across the asserts
        // (clippy::significant_drop_tightening).
        let bodies = receiver.captured.lock().expect("lock").clone();
        let records = bodies[0]["resourceLogs"][0]["scopeLogs"][0]["logRecords"]
            .as_array()
            .expect("logRecords array");
        assert_eq!(records.len(), 2);
        // The body and identity survive the projection onto the log record.
        assert_eq!(records[0]["body"]["stringValue"], "ls");
        let has_external_id = records[0]["attributes"]
            .as_array()
            .expect("attributes")
            .iter()
            .any(|kv| kv["key"] == "external_id" && kv["value"]["stringValue"] == "a");
        assert!(has_external_id, "external_id must cross the bus as an attribute");
    }

    #[tokio::test]
    async fn empty_source_emits_nothing() {
        let receiver = spawn_receiver().await;
        let adapter = TestSource { docs: vec![] };
        let report = sync(&adapter, &Config { endpoint: receiver.endpoint }).await.expect("sync");
        assert!(report.skipped);
        assert_eq!(report.records, 0);
        assert!(receiver.captured.lock().expect("lock").is_empty());
    }
}
