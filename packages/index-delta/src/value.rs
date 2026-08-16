//! Format detection and parsing: every structured format is lifted to a
//! `serde_json::Value` so a single differ (`diff.rs`) serves all of them.
//! Parsing is lossy where a format has types JSON lacks (TOML datetimes,
//! plist data/dates become strings); that is fine for *diffing* — `apply.rs`
//! goes back through each format's own writer, not through this
//! normalization.

use std::fmt;
use std::io::Cursor;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// File formats the logical-diff engines understand. `Text` is the fallback:
/// still tracked and journaled, but diffed as unified hunks rather than
/// addressed ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Json,
    Toml,
    Yaml,
    Plist,
    /// Line-oriented `key = value` files with optional `[section]` headers
    /// (ghostty, git config, INI). Unlike TOML it tolerates repeated keys,
    /// which become arrays.
    Keyvalue,
    Text,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Single source of truth for the lowercase names: the ValueEnum
        // derive (which also keeps Display in sync with the CLI's accepted
        // `--format` spellings and the serde rename above).
        let value = clap::ValueEnum::to_possible_value(self).expect("no skipped variants");
        f.write_str(value.get_name())
    }
}

/// A file's contents lifted to the representation its format is diffed in.
pub enum Doc {
    Structured(Value),
    Text(String),
    Binary(Vec<u8>),
}

/// Pick a format for a file: extension first, then content sniffing on the
/// base. Runs once when a file is first managed; the result is recorded in
/// its meta so detection can never flip under an existing diff.
pub fn detect(path: &Path, contents: &[u8]) -> Format {
    by_extension(path).unwrap_or_else(|| sniff(contents))
}

fn by_extension(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "json" => Some(Format::Json),
        "toml" => Some(Format::Toml),
        "yaml" | "yml" => Some(Format::Yaml),
        "plist" => Some(Format::Plist),
        "ini" => Some(Format::Keyvalue),
        _ => None,
    }
}

fn sniff(contents: &[u8]) -> Format {
    if contents.starts_with(b"bplist") {
        return Format::Plist;
    }
    let Ok(text) = std::str::from_utf8(contents) else {
        return Format::Text;
    };
    let first = text.trim_start().chars().next();
    if matches!(first, Some('{' | '[')) && serde_json::from_str::<Value>(text).is_ok() {
        return Format::Json;
    }
    if text.contains("<plist") {
        return Format::Plist;
    }
    // TOML before the keyvalue heuristic: a file that parses as TOML gets the
    // format-preserving toml_edit writer for `apply-ops`. Files TOML rejects
    // but that still look line-oriented (repeated keys, `key: value`) fall
    // through to keyvalue.
    if text.contains('=') && toml::from_str::<toml::Value>(text).is_ok() {
        return Format::Toml;
    }
    if looks_keyvalue(text) {
        return Format::Keyvalue;
    }
    Format::Text
}

fn looks_keyvalue(text: &str) -> bool {
    let significant = text
        .lines()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !t.starts_with('#') && !t.starts_with(';'));
    let (entries, other): (Vec<_>, Vec<_>) = significant.partition(|t| keyvalue_entry(t));
    !entries.is_empty() && entries.len() >= other.len()
}

/// A section header (`[name]`) or a `key=value` line.
fn keyvalue_entry(trimmed: &str) -> bool {
    (trimmed.starts_with('[') && trimmed.ends_with(']')) || trimmed.contains('=')
}

/// Parse contents under a format, degrading gracefully: a structured file
/// that fails to parse (e.g. an app saved mid-edit garbage) is treated as
/// text so drift is still visible instead of erroring the whole run.
pub fn load(format: Format, contents: &[u8]) -> Doc {
    let structured = match format {
        Format::Json => std::str::from_utf8(contents)
            .ok()
            .and_then(|text| serde_json::from_str(text).ok()),
        Format::Toml => std::str::from_utf8(contents)
            .ok()
            .and_then(|text| toml::from_str::<toml::Value>(text).ok())
            .map(toml_to_json),
        Format::Yaml => std::str::from_utf8(contents)
            .ok()
            .and_then(|text| serde_norway::from_str::<serde_norway::Value>(text).ok())
            .and_then(|yaml| serde_json::to_value(yaml).ok()),
        Format::Plist => plist::Value::from_reader(Cursor::new(contents))
            .ok()
            .map(plist_to_json),
        Format::Keyvalue => std::str::from_utf8(contents).ok().map(parse_keyvalue),
        Format::Text => None,
    };
    structured.map_or_else(|| text_or_binary(contents), Doc::Structured)
}

fn text_or_binary(contents: &[u8]) -> Doc {
    std::str::from_utf8(contents).map_or_else(
        |_| Doc::Binary(contents.to_vec()),
        |text| Doc::Text(text.to_owned()),
    )
}

fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::from(i),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map_or_else(|| Value::String(f.to_string()), Value::Number),
        toml::Value::Boolean(b) => Value::from(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .into_iter()
                .map(|(key, item)| (key, toml_to_json(item)))
                .collect(),
        ),
    }
}

pub fn plist_to_json(value: plist::Value) -> Value {
    match value {
        plist::Value::String(s) => Value::String(s),
        plist::Value::Boolean(b) => Value::from(b),
        plist::Value::Integer(i) => i
            .as_signed()
            .map(Value::from)
            .or_else(|| i.as_unsigned().map(Value::from))
            .unwrap_or(Value::Null),
        plist::Value::Real(f) => serde_json::Number::from_f64(f)
            .map_or_else(|| Value::String(f.to_string()), Value::Number),
        plist::Value::Date(d) => Value::String(d.to_xml_format()),
        plist::Value::Data(bytes) => Value::String(format!("hex:{}", crate::store::hex(&bytes))),
        plist::Value::Uid(uid) => Value::from(uid.get()),
        plist::Value::Array(items) => Value::Array(items.into_iter().map(plist_to_json).collect()),
        plist::Value::Dictionary(dict) => Value::Object(
            dict.into_iter()
                .map(|(key, item)| (key, plist_to_json(item)))
                .collect(),
        ),
        _ => Value::Null,
    }
}

pub fn parse_keyvalue(text: &str) -> Value {
    let mut root = Map::new();
    let mut section: Option<String> = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(name) = header_name(trimmed) {
            root.entry(name.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            section = Some(name);
            continue;
        }
        let entry = split_entry(trimmed);
        let map = match &section {
            None => &mut root,
            Some(name) => {
                let slot = root
                    .entry(name.clone())
                    .or_insert_with(|| Value::Object(Map::new()));
                // A bare key earlier claimed this name; entries after the
                // header win and the scalar is dropped.
                if !slot.is_object() {
                    *slot = Value::Object(Map::new());
                }
                slot.as_object_mut().expect("just ensured an object")
            }
        };
        insert_multi(map, entry.key, entry.value);
    }
    Value::Object(root)
}

pub fn header_name(trimmed: &str) -> Option<String> {
    trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .map(|name| name.trim().to_owned())
}

/// A parsed keyvalue entry line.
pub struct Entry<'line> {
    pub key: &'line str,
    pub value: &'line str,
}

/// Split a keyvalue entry line. Lines without `=` are flag-style bare keys
/// with an empty value.
pub fn split_entry(trimmed: &str) -> Entry<'_> {
    trimmed.split_once('=').map_or(
        Entry {
            key: trimmed,
            value: "",
        },
        |(key, value)| Entry {
            key: key.trim_end(),
            value: value.trim_start(),
        },
    )
}

fn insert_multi(map: &mut Map<String, Value>, key: &str, value: &str) {
    let new = Value::String(value.to_owned());
    match map.get_mut(key) {
        None => {
            map.insert(key.to_owned(), new);
        }
        Some(Value::Array(items)) => items.push(new),
        Some(existing) => {
            let first = existing.take();
            *existing = Value::Array(vec![first, new]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_by_extension_then_content() {
        assert_eq!(detect(Path::new("a.json"), b"whatever"), Format::Json);
        assert_eq!(detect(Path::new("config"), b"{\"a\": 1}"), Format::Json);
        assert_eq!(
            detect(Path::new("config"), b"a = 1\n[table]\nb = 2\n"),
            Format::Toml
        );
        assert_eq!(
            detect(Path::new("config"), b"keybind = a\nkeybind = b\n"),
            Format::Keyvalue
        );
        assert_eq!(detect(Path::new("notes"), b"just prose\n"), Format::Text);
        assert_eq!(detect(Path::new("d"), b"bplist00garbage"), Format::Plist);
    }

    #[test]
    fn keyvalue_sections_and_repeats() {
        let parsed = parse_keyvalue("top = 1\nkeybind = a\nkeybind = b\n[sec]\nk = v\n");
        assert_eq!(
            parsed,
            serde_json::json!({
                "top": "1",
                "keybind": ["a", "b"],
                "sec": {"k": "v"},
            })
        );
    }

    #[test]
    fn corrupt_structured_falls_back_to_text() {
        let doc = load(Format::Json, b"{not json");
        assert!(matches!(doc, Doc::Text(_)));
    }
}
