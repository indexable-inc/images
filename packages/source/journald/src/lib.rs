//! Adapter turning journald unit logs into embeddable, tagged [`source_meta`]
//! documents for the multi-source `search` store.
//!
//! # Grain
//! One [`Document`] per **(unit, UTC day)**, embedding only that day's
//! priority<=4 messages (warning and worse) as `HH:MM:SS [level] message`
//! lines. The journal's volume makes a per-message grain untenable, and the
//! info/debug levels carry no incident signal; the warning-and-worse day slice
//! is what an agent diagnosing a unit failure actually needs. Each document is
//! capped ([`MAX_MESSAGES`] messages / [`MAX_BODY_BYTES`] bytes) so a
//! crash-looping unit cannot dominate the store.
//! `external_id = "journald:{host}:{unit}:{date}"`, so a re-sync re-uploads
//! only the still-growing current day; past days are stable and skipped by the
//! content-hash gate.
//!
//! # Tags
//! Every document carries the common header (`source`, `external_id`,
//! `content_hash`, `title`, `timestamp`) plus `host` and `unit`, so a query can
//! scope to a machine, a unit, or a time range.
//!
//! # Reading
//! [`JournaldLog::read`] shells out to
//! `journalctl -o json --priority 4 --since <timespec>` (the fleet indexer runs
//! as root, which reads the full system journal). A malformed JSON line is a
//! typed error; a well-formed entry missing `PRIORITY` or its timestamp is
//! skipped by design — those are journal oddities, not corruption — and message
//! text is sanitized (ANSI stripped, credential shapes redacted, blobs
//! collapsed) before grouping, hashing, and embedding.

#![forbid(unsafe_code)]

mod error;
mod record;

use std::collections::BTreeMap;
use std::process::Command;

use snafu::ResultExt as _;
use source_meta::{Document, Source, SourceAdapter};

pub use crate::error::Error;
use crate::error::{JournalctlFailedSnafu, ParseSnafu, Result, SpawnSnafu};
pub use crate::record::{MAX_BODY_BYTES, MAX_MESSAGES, Message, UnitDay};

/// The `source` tag every journald document carries.
pub const SOURCE_TAG: &str = "journald";

/// Highest syslog priority value ingested (4 = warning; lower is worse).
pub const MAX_PRIORITY: u8 = 4;

/// A window of journald unit logs grouped per (unit, day), ready to project
/// into documents.
///
/// Construct with [`JournaldLog::read`] (shells out to `journalctl` once) or
/// [`JournaldLog::parse`] for captured `-o json` lines (tests, fixtures).
#[derive(Debug)]
#[must_use]
pub struct JournaldLog {
    days: Vec<UnitDay>,
}

impl JournaldLog {
    /// Read priority<=4 journal entries since `since` (a systemd timespec like
    /// `2026-06-01`, `yesterday`, or a bare duration like `2d`, which is
    /// normalized to the `-2d` form journalctl expects).
    ///
    /// # Errors
    /// Returns an error if `journalctl` cannot be spawned, exits non-zero, or
    /// emits a malformed JSON line.
    pub fn read(since: &str, host: &str) -> Result<Self> {
        let since = normalize_since(since);
        let output = Command::new("journalctl")
            .args([
                "-o",
                "json",
                "--priority",
                "4",
                "--no-pager",
                "--since",
                &since,
            ])
            .output()
            .context(SpawnSnafu)?;
        if !output.status.success() {
            return JournalctlFailedSnafu {
                since,
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
            .fail();
        }
        Self::parse(&output.stdout, host)
    }

    /// Parse captured `journalctl -o json` output (one JSON object per line)
    /// and group it per (unit, UTC day).
    ///
    /// # Errors
    /// Returns an error if a non-empty line is not a JSON object.
    pub fn parse(bytes: &[u8], host: &str) -> Result<Self> {
        let text = String::from_utf8_lossy(bytes);
        let mut groups: BTreeMap<(String, String), Vec<Message>> = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let entry: serde_json::Value = serde_json::from_str(line).map_err(|error| {
                ParseSnafu {
                    detail: format!("{error}: {}", &line[..line.len().min(120)]),
                }
                .build()
            })?;
            let Some(parsed) = parse_entry(&entry) else {
                continue;
            };
            let ParsedEntry {
                unit,
                date,
                message,
            } = parsed;
            groups.entry((unit, date)).or_default().push(message);
        }

        let days = groups
            .into_iter()
            .map(|((unit, date), mut messages)| {
                // journalctl emits in time order, but the grouping must not
                // depend on it (merged boot logs can interleave).
                messages.sort_by_key(|message| message.timestamp);
                UnitDay {
                    host: host.to_owned(),
                    unit,
                    date,
                    messages,
                }
            })
            .collect();
        Ok(Self { days })
    }

    /// Number of (unit, day) groups.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.days.len()
    }

    /// Whether no entries survived the priority filter.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.days.is_empty()
    }
}

impl SourceAdapter for JournaldLog {
    type Error = Error;

    fn source(&self) -> Source {
        Source::new(SOURCE_TAG)
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Clone into an owned iterator so the result is `'static + Send`,
        // independent of `&self` (mirrors the other source adapters).
        self.days.clone().into_iter().map(UnitDay::into_document)
    }
}

/// One journal line reduced to its grouping key and message.
struct ParsedEntry {
    unit: String,
    date: String,
    message: Message,
}

/// Reduce one `journalctl -o json` object to a [`ParsedEntry`], or `None` when
/// it should be skipped: priority absent/unparseable/over [`MAX_PRIORITY`]
/// (defense in depth behind journalctl's own `--priority 4`), timestamp absent,
/// or message empty after sanitation.
fn parse_entry(entry: &serde_json::Value) -> Option<ParsedEntry> {
    let priority: u8 = entry.get("PRIORITY")?.as_str()?.parse().ok()?;
    if priority > MAX_PRIORITY {
        return None;
    }
    let micros: i64 = entry.get("__REALTIME_TIMESTAMP")?.as_str()?.parse().ok()?;
    let timestamp = micros / 1_000_000;
    let date = chrono::DateTime::from_timestamp(timestamp, 0)?
        .date_naive()
        .to_string();

    let raw = message_text(entry)?;
    // Sanitize per message, before grouping: unit logs capture command output
    // and API traffic, so they are a direct secret-leak path into the store.
    // Sanitizing here (not after the body cap) keeps the document caps exact.
    let text = source_meta::sanitize::sanitize(raw.trim_end());
    if text.trim().is_empty() {
        return None;
    }

    Some(ParsedEntry {
        unit: unit_of(entry),
        date,
        message: Message {
            timestamp,
            priority,
            text,
        },
    })
}

/// The journal `MESSAGE` field as text. journald stores non-UTF-8 payloads as
/// a JSON array of bytes; those are recovered lossily rather than dropped.
fn message_text(entry: &serde_json::Value) -> Option<String> {
    match entry.get("MESSAGE")? {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(bytes) => {
            let buffer: Vec<u8> = bytes
                .iter()
                .filter_map(serde_json::Value::as_u64)
                .filter_map(|byte| u8::try_from(byte).ok())
                .collect();
            Some(String::from_utf8_lossy(&buffer).into_owned())
        }
        _ => None,
    }
}

/// The unit a journal entry belongs to: the owning systemd unit when known,
/// else the unit a pid1 state-change message is *about* (`UNIT`), else the
/// syslog identifier (kernel messages land here as `kernel`), else the
/// transport, else `unknown`. Attribution drives the document grain, so the
/// chain prefers the most specific stable name available.
fn unit_of(entry: &serde_json::Value) -> String {
    for key in ["_SYSTEMD_UNIT", "UNIT", "SYSLOG_IDENTIFIER", "_TRANSPORT"] {
        if let Some(value) = entry.get(key).and_then(serde_json::Value::as_str)
            && !value.is_empty()
        {
            return value.to_owned();
        }
    }
    "unknown".to_owned()
}

/// Normalize a `--since` timespec for journalctl.
///
/// journalctl rejects a bare relative duration (`2d`, `12h`, `90min`) — it
/// wants `-2d` — so the `-` is prepended for the duration shape (alphanumeric
/// with at least one digit and one letter). Dates, words (`yesterday`), and
/// already-signed specs pass through.
#[must_use]
pub fn normalize_since(since: &str) -> String {
    let is_bare_duration = since.chars().all(|c| c.is_ascii_alphanumeric())
        && since.chars().any(|c| c.is_ascii_digit())
        && since.chars().any(|c| c.is_ascii_alphabetic());
    if is_bare_duration {
        format!("-{since}")
    } else {
        since.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_since;

    #[test]
    fn bare_durations_get_a_sign_and_other_specs_pass_through() {
        assert_eq!(normalize_since("2d"), "-2d");
        assert_eq!(normalize_since("12h"), "-12h");
        assert_eq!(normalize_since("1h30m"), "-1h30m");
        assert_eq!(normalize_since("-2d"), "-2d");
        assert_eq!(normalize_since("yesterday"), "yesterday");
        assert_eq!(normalize_since("2026-06-01"), "2026-06-01");
        assert_eq!(normalize_since("2 days ago"), "2 days ago");
    }
}
