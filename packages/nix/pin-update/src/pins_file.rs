//! pins.json IO. Entries keep their on-disk order (a re-pin rewrites hashes,
//! not layout), so both map levels are `IndexMap`s; fields beyond the ones an
//! updater owns pass through as opaque `serde_json::Value`s.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

pub type Entry = IndexMap<String, Value>;
pub type Pins = IndexMap<String, Entry>;

pub fn read(path: &Path) -> Result<Pins> {
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

/// Write `value` as 2-space-indented JSON with a trailing newline (the repo
/// convention for generated JSON).
pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut rendered = serde_json::to_string_pretty(value)
        .with_context(|| format!("serializing {}", path.display()))?;
    rendered.push('\n');
    fs::write(path, rendered).with_context(|| format!("writing {}", path.display()))
}

/// A string-valued field of an entry, `None` when absent or non-string.
pub fn str_field<'entry>(entry: &'entry Entry, key: &str) -> Option<&'entry str> {
    entry.get(key).and_then(Value::as_str)
}

/// Set `key` to a string value, preserving its position when it exists.
pub fn set_str(entry: &mut Entry, key: &str, value: String) {
    entry.insert(key.to_owned(), Value::String(value));
}
