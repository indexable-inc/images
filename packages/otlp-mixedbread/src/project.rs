//! Projection of one OTLP [`LogRecord`] into a Mixedbread [`Document`].
//!
//! The embedded body is `"<label>: <message>"` so a semantic query matches on the
//! unit/service as well as the text. The `external_id` is a content hash over the
//! record's identity (time + host + unit + body), so re-delivery of the same
//! record (a collector retry) is idempotent: it maps to the same store id and
//! overwrites rather than duplicating. A log record is immutable, so its
//! `content_hash` is stable too.

use source_meta::{Document, keys};
use serde_json::{Map, Value, json};

use crate::otlp::{Attrs, LogRecord, Resource, severity_label, severity_number};

/// Nanoseconds per second; OTLP timestamps are nanoseconds since the epoch.
const NANOS_PER_SEC: i64 = 1_000_000_000;

/// A record reduced to the fields we store, with the merged attribute view.
struct Fields {
    body: String,
    host: Option<String>,
    unit: Option<String>,
    identifier: Option<String>,
    service_name: Option<String>,
    pid: Option<i64>,
    boot_id: Option<String>,
    severity: Option<String>,
    timestamp: Option<i64>,
}

impl Fields {
    /// The display label for the log's origin: unit, else program identifier,
    /// else service name, else a generic tag.
    fn label(&self) -> &str {
        self.unit
            .as_deref()
            .or(self.identifier.as_deref())
            .or(self.service_name.as_deref())
            .unwrap_or("log")
    }
}

/// Project one record (with its resource) into a [`Document`], or `None` when it
/// has no body to embed or its metadata would exceed the store limits. The
/// `source` tag scopes the corpus (e.g. `"log"`).
#[must_use]
pub fn project(resource: Option<&Resource>, record: &LogRecord, source: &str) -> Option<Document> {
    let attrs = Attrs::merge(resource, record);
    let body = record.body.as_ref().and_then(|value| value.render())?;
    if body.trim().is_empty() {
        return None;
    }

    let severity = record.severity_text.clone().or_else(|| {
        severity_number(record.severity_number.as_ref()).map(|n| severity_label(n).to_owned())
    });
    let fields = Fields {
        body,
        host: attrs.first(&["host.name", "_HOSTNAME", "host"]),
        unit: attrs.first(&["_SYSTEMD_UNIT", "systemd.unit", "_SYSTEMD_USER_UNIT"]),
        identifier: attrs.first(&["SYSLOG_IDENTIFIER", "syslog.identifier", "_COMM"]),
        service_name: attrs.first(&["service.name"]),
        pid: attrs.first(&["_PID", "process.pid"]).and_then(|value| value.parse().ok()),
        boot_id: attrs.first(&["_BOOT_ID"]),
        severity,
        timestamp: record
            .time_unix_nano
            .as_deref()
            .or(record.observed_time_unix_nano.as_deref())
            .and_then(|nanos| nanos.parse::<i64>().ok())
            .map(|nanos| nanos / NANOS_PER_SEC),
    };

    document(&fields, source)
}

/// Build the [`Document`] from reduced fields, or `None` if metadata is too big.
fn document(fields: &Fields, source: &str) -> Option<Document> {
    let label = fields.label();
    let embedded = format!("{label}: {}", fields.body);
    let content_hash = source_meta::hash_body(embedded.as_bytes());
    let external_id = external_id(fields, label);

    let mut meta = Map::new();
    meta.insert(keys::SOURCE.to_owned(), json!(source));
    meta.insert("external_id".to_owned(), json!(external_id));
    meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
    meta.insert(keys::TITLE.to_owned(), json!(title(fields, label)));
    insert_some(&mut meta, keys::HOST, fields.host.clone().map(Value::from));
    insert_some(&mut meta, keys::UNIT, fields.unit.clone().map(Value::from));
    insert_some(&mut meta, keys::SYSLOG_IDENTIFIER, fields.identifier.clone().map(Value::from));
    insert_some(&mut meta, keys::SERVICE_NAME, fields.service_name.clone().map(Value::from));
    insert_some(&mut meta, keys::SEVERITY, fields.severity.clone().map(Value::from));
    insert_some(&mut meta, keys::PID, fields.pid.map(Value::from));
    insert_some(&mut meta, keys::BOOT_ID, fields.boot_id.clone().map(Value::from));
    insert_some(&mut meta, keys::TIMESTAMP, fields.timestamp.map(Value::from));
    let meta_json = Value::Object(meta);

    if let Err(error) = source_meta::check_metadata(&external_id, &meta_json) {
        // A pathological record (huge attribute) is dropped, not fatal; the rest
        // of the batch still flows.
        tracing::warn!(%external_id, %error, "dropping log record: metadata over limit");
        return None;
    }

    Some(Document {
        external_id,
        file_name: format!("{}.txt", &content_hash["sha256:".len()..]),
        mime: "text/plain",
        body: embedded.into_bytes(),
        meta_json,
        content_hash,
    })
}

/// A stable, unique id for the record: `log:<sha256 of identity>`. The identity
/// is time + host + unit + body, so two deliveries of one record collide (good:
/// idempotent overwrite) while distinct records do not.
fn external_id(fields: &Fields, label: &str) -> String {
    let timestamp = fields.timestamp.unwrap_or(0);
    let host = fields.host.as_deref().unwrap_or("");
    let identity = format!("{timestamp}\u{1f}{host}\u{1f}{label}\u{1f}{}", fields.body);
    format!("log:{}", source_meta::hash_body(identity.as_bytes()))
}

/// A short human title: `log[<severity>] <label>: <first line>` capped.
fn title(fields: &Fields, label: &str) -> String {
    let severity = fields.severity.as_deref().unwrap_or("");
    let snippet: String = fields
        .body
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(80)
        .collect();
    format!("log[{severity}] {label}: {snippet}")
}

/// Insert `key` only when a value is present, keeping absent tags off the record.
fn insert_some(meta: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        meta.insert(key.to_owned(), value);
    }
}
