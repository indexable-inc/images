//! `otlp-mixedbread`: an OTLP/HTTP logs receiver that maps each log record into a
//! Mixedbread document for semantic search.
//!
//! It is the bridge that lets Mixedbread sit at the OpenTelemetry collector's
//! exporter layer: the collector's `otlphttp` exporter (JSON encoding) points at
//! this service, which projects every [`otlp::LogRecord`] into a
//! [`source_meta::Document`] and reconciles it into a Mixedbread store via
//! `search-core`. Where the logs come from (a journald receiver, a `filelog`
//! receiver, an app's OTLP SDK) is the collector's concern, so this stays one
//! generic ingress for any OTLP log source.
//!
//! See [`server::router`] for the HTTP surface and [`ingest`] for the async
//! upload pipeline.

#![forbid(unsafe_code)]

pub mod ingest;
pub mod otlp;
pub mod project;
pub mod server;

pub use ingest::{Config, Ingest, Sent, spawn};
pub use server::{AppState, router};

#[cfg(test)]
mod tests {
    use source_meta::keys;

    use crate::otlp::ExportLogsServiceRequest;
    use crate::project::project;

    // A journald-shaped OTLP/JSON export: the journald receiver puts `MESSAGE`
    // in the body and the journal fields in record attributes, with `host.name`
    // and `service.name` in the resource. `intValue` is a string (proto3 JSON);
    // `severityNumber` is the integer form.
    const SAMPLE: &str = r#"{
      "resourceLogs": [{
        "resource": { "attributes": [
          { "key": "host.name", "value": { "stringValue": "node-1" } },
          { "key": "service.name", "value": { "stringValue": "nginx" } }
        ] },
        "scopeLogs": [{
          "logRecords": [{
            "timeUnixNano": "1700000000000000000",
            "severityNumber": 17,
            "severityText": "ERROR",
            "body": { "stringValue": "upstream timed out" },
            "attributes": [
              { "key": "_SYSTEMD_UNIT", "value": { "stringValue": "nginx.service" } },
              { "key": "SYSLOG_IDENTIFIER", "value": { "stringValue": "nginx" } },
              { "key": "_PID", "value": { "intValue": "4242" } }
            ]
          }]
        }]
      }]
    }"#;

    fn first_document(json: &str, source: &str) -> source_meta::Document {
        let request: ExportLogsServiceRequest = serde_json::from_str(json).expect("parse OTLP json");
        let resource_logs = request.resource_logs.first().expect("one resourceLogs");
        let record = resource_logs.scope_logs[0].log_records.first().expect("one record");
        project(resource_logs.resource.as_ref(), record, source).expect("a document")
    }

    #[test]
    fn projects_journald_record_with_structured_tags() {
        let doc = first_document(SAMPLE, "log");
        // Unit is embedded so a query can match on it, not just the message.
        assert_eq!(doc.body, b"nginx.service: upstream timed out");
        let meta = doc.meta_json.as_object().expect("object");
        assert_eq!(meta[keys::SOURCE], "log");
        assert_eq!(meta[keys::UNIT], "nginx.service");
        assert_eq!(meta[keys::SERVICE_NAME], "nginx");
        assert_eq!(meta[keys::SEVERITY], "ERROR");
        assert_eq!(meta[keys::HOST], "node-1");
        assert_eq!(meta[keys::PID], 4242);
        assert_eq!(meta[keys::TIMESTAMP], 1_700_000_000);
        assert!(doc.external_id.starts_with("log:sha256:"));
    }

    #[test]
    fn re_delivery_is_idempotent() {
        // Two projections of the same record must produce the same store id, so a
        // collector retry overwrites rather than duplicates.
        let a = first_document(SAMPLE, "log");
        let b = first_document(SAMPLE, "log");
        assert_eq!(a.external_id, b.external_id);
        assert_eq!(a.content_hash, b.content_hash);
    }

    #[test]
    fn empty_body_record_is_skipped() {
        let json = r#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[
          {"body":{"stringValue":"   "}},
          {"severityText":"INFO"}
        ]}]}]}"#;
        let request: ExportLogsServiceRequest = serde_json::from_str(json).expect("parse");
        let records = &request.resource_logs[0].scope_logs[0].log_records;
        assert!(project(None, &records[0], "log").is_none(), "blank body skipped");
        assert!(project(None, &records[1], "log").is_none(), "missing body skipped");
    }

    #[test]
    fn severity_number_accepts_enum_name() {
        // proto3 JSON may encode the enum as its name instead of the integer.
        let json = r#"{"resourceLogs":[{"scopeLogs":[{"logRecords":[
          {"severityNumber":"SEVERITY_NUMBER_WARN","body":{"stringValue":"disk low"}}
        ]}]}]}"#;
        let request: ExportLogsServiceRequest = serde_json::from_str(json).expect("parse");
        let record = &request.resource_logs[0].scope_logs[0].log_records[0];
        let doc = project(None, record, "log").expect("a document");
        assert_eq!(doc.meta_json[keys::SEVERITY], "WARN");
    }
}
