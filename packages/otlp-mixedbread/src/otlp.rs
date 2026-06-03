//! The subset of the OTLP/JSON logs schema we consume, plus helpers to flatten
//! it into the shape the projection needs.
//!
//! We accept OTLP over HTTP with `encoding: json` (the collector's `otlphttp`
//! exporter), so this is the protobuf-JSON mapping of `ExportLogsServiceRequest`
//! ([opentelemetry-proto]), not protobuf. Only the fields we project are typed;
//! everything else is ignored by serde. A field that proto3-JSON may encode as
//! either a number or a string (an int64, an enum) is kept as a [`serde_json::Value`]
//! and normalized in code rather than fighting serde over the dual encoding.
//!
//! [opentelemetry-proto]: https://github.com/open-telemetry/opentelemetry-proto/blob/main/opentelemetry/proto/collector/logs/v1/logs_service.proto

use std::collections::HashMap;

use serde::Deserialize;

/// Top-level OTLP logs export request body.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExportLogsServiceRequest {
    #[serde(default)]
    pub resource_logs: Vec<ResourceLogs>,
}

/// Logs from one resource (a process/host), with its resource attributes.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLogs {
    pub resource: Option<Resource>,
    #[serde(default)]
    pub scope_logs: Vec<ScopeLogs>,
}

/// A resource descriptor; we only read its attributes.
#[derive(Debug, Deserialize, Default)]
pub struct Resource {
    #[serde(default)]
    pub attributes: Vec<KeyValue>,
}

/// Logs from one instrumentation scope.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScopeLogs {
    #[serde(default)]
    pub log_records: Vec<LogRecord>,
}

/// One log line and its structured fields.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    /// Event time, nanoseconds since the Unix epoch, as a string (proto3 JSON).
    #[serde(default)]
    pub time_unix_nano: Option<String>,
    /// When the collector observed the record; the fallback time.
    #[serde(default)]
    pub observed_time_unix_nano: Option<String>,
    /// OTLP severity number 1..=24 (higher is more severe); a number or enum name.
    #[serde(default)]
    pub severity_number: Option<serde_json::Value>,
    /// Human severity text (`ERROR`, `WARN`, ...), when set by the source.
    #[serde(default)]
    pub severity_text: Option<String>,
    /// The log body; usually a `stringValue`.
    pub body: Option<AnyValue>,
    /// Record-level attributes (journald fields land here).
    #[serde(default)]
    pub attributes: Vec<KeyValue>,
}

/// One attribute: a key and its (optional) value.
#[derive(Debug, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: Option<AnyValue>,
}

/// OTLP `AnyValue`: exactly one of these fields is set. We render it to a string
/// for metadata and bodies; nested array/kvlist values keep their JSON form.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AnyValue {
    pub string_value: Option<String>,
    pub bool_value: Option<bool>,
    pub int_value: Option<serde_json::Value>,
    pub double_value: Option<f64>,
    pub bytes_value: Option<String>,
    pub array_value: Option<serde_json::Value>,
    pub kvlist_value: Option<serde_json::Value>,
}

impl AnyValue {
    /// The value as a display string, or `None` when no field is set.
    #[must_use]
    pub fn render(&self) -> Option<String> {
        if let Some(text) = &self.string_value {
            return Some(text.clone());
        }
        if let Some(flag) = self.bool_value {
            return Some(flag.to_string());
        }
        if let Some(int) = &self.int_value {
            return Some(scalar_string(int));
        }
        if let Some(float) = self.double_value {
            return Some(float.to_string());
        }
        if let Some(array) = &self.array_value {
            return Some(array.to_string());
        }
        if let Some(kvlist) = &self.kvlist_value {
            return Some(kvlist.to_string());
        }
        self.bytes_value.clone()
    }
}

/// A `serde_json` scalar as a bare string: the inner text for a JSON string, the
/// numeric literal for a number, else its JSON form. Used because proto3 JSON
/// encodes int64 as a string but some encoders still emit a number.
fn scalar_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// A flat, case-sensitive view of one record's attributes (resource attributes
/// merged with record attributes, record winning), plus first-match lookup over
/// candidate keys so a caller can try several spellings of the same field.
pub struct Attrs {
    map: HashMap<String, String>,
}

impl Attrs {
    /// Merge resource then record attributes into one lookup. Record attributes
    /// override resource ones on a key clash.
    #[must_use]
    pub fn merge(resource: Option<&Resource>, record: &LogRecord) -> Self {
        let mut map = HashMap::new();
        if let Some(resource) = resource {
            insert_all(&mut map, &resource.attributes);
        }
        insert_all(&mut map, &record.attributes);
        Self { map }
    }

    /// The first candidate key that is present, or `None`.
    #[must_use]
    pub fn first(&self, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|key| self.map.get(*key).cloned())
    }
}

/// Insert every renderable attribute into `map`.
fn insert_all(map: &mut HashMap<String, String>, attributes: &[KeyValue]) {
    for attribute in attributes {
        if let Some(rendered) = attribute.value.as_ref().and_then(AnyValue::render) {
            map.insert(attribute.key.clone(), rendered);
        }
    }
}

/// The numeric value of an OTLP `severityNumber`, whether encoded as an integer
/// or the proto enum name (`SEVERITY_NUMBER_ERROR` -> 17). `None` when absent or
/// unrecognized.
#[must_use]
pub fn severity_number(value: Option<&serde_json::Value>) -> Option<i32> {
    let value = value?;
    if let Some(int) = value.as_i64() {
        return i32::try_from(int).ok();
    }
    let name = value.as_str()?;
    // SeverityNumber enum: 4 tiers of 4, TRACE..FATAL. The suffix names map to a
    // base; the numbered variants (`..._ERROR2`) add an offset.
    let (base, tier) = match name.trim_start_matches("SEVERITY_NUMBER_") {
        n if n.starts_with("TRACE") => (1, n.strip_prefix("TRACE")),
        n if n.starts_with("DEBUG") => (5, n.strip_prefix("DEBUG")),
        n if n.starts_with("INFO") => (9, n.strip_prefix("INFO")),
        n if n.starts_with("WARN") => (13, n.strip_prefix("WARN")),
        n if n.starts_with("ERROR") => (17, n.strip_prefix("ERROR")),
        n if n.starts_with("FATAL") => (21, n.strip_prefix("FATAL")),
        _ => return None,
    };
    // "" or "2".."4" -> offset 0..3.
    let offset = tier.and_then(|t| t.parse::<i32>().ok()).map_or(0, |n| n - 1);
    Some(base + offset.clamp(0, 3))
}

/// The standard short label for an OTLP severity number, for titles when the
/// source did not set `severityText`.
#[must_use]
pub fn severity_label(number: i32) -> &'static str {
    match number {
        1..=4 => "TRACE",
        5..=8 => "DEBUG",
        9..=12 => "INFO",
        13..=16 => "WARN",
        17..=20 => "ERROR",
        n if n >= 21 => "FATAL",
        _ => "UNSPECIFIED",
    }
}
