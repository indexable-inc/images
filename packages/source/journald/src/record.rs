//! The typed per-(unit, day) record and its projection to a search [`Document`].

use serde_json::{Map, Value, json};
use snafu::ResultExt as _;
use source_meta::{Document, keys};

use crate::SOURCE_TAG;
use crate::error::{MetadataSnafu, Result};

/// Maximum messages one (unit, day) document embeds.
///
/// The rest are summarized by a `[truncated ...]` trailer: a crash-looping unit
/// can log the same error tens of thousands of times a day, and past the first
/// couple hundred lines the extra repeats add embedding cost without adding
/// retrievable signal.
pub const MAX_MESSAGES: usize = 200;

/// Maximum body size in bytes one (unit, day) document embeds (the message cap's
/// byte-denominated twin, for units that log few but huge messages).
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// One journald message that survived the priority filter, already sanitized.
#[derive(Debug, Clone)]
pub struct Message {
    /// Epoch seconds of the journal entry (`__REALTIME_TIMESTAMP` / 1e6).
    pub timestamp: i64,
    /// syslog priority (0 emerg .. 4 warning; higher ones are filtered out).
    pub priority: u8,
    /// Sanitized message text.
    pub text: String,
}

/// One (unit, day) group: the grain of a journald document.
#[derive(Debug, Clone)]
pub struct UnitDay {
    /// Host the journal was read on.
    pub host: String,
    /// systemd unit name (or the syslog identifier / transport fallback).
    pub unit: String,
    /// UTC day, `YYYY-MM-DD`.
    pub date: String,
    /// The day's priority<=4 messages, in time order.
    pub messages: Vec<Message>,
}

/// Human label for a syslog priority level (only 0..=4 survive the filter).
const fn priority_label(priority: u8) -> &'static str {
    match priority {
        0 => "emerg",
        1 => "alert",
        2 => "crit",
        3 => "err",
        _ => "warning",
    }
}

impl UnitDay {
    /// Stable store id: `journald:{host}:{unit}:{date}`. Per (unit, day), so a
    /// re-sync re-uploads only days whose message set changed (today's document
    /// grows until midnight; past days are stable and skipped by the
    /// content-hash gate).
    #[must_use]
    pub fn external_id(&self) -> String {
        format!("journald:{}:{}:{}", self.host, self.unit, self.date)
    }

    /// Render the embeddable body: one `HH:MM:SS [level] message` line per
    /// message, capped at [`MAX_MESSAGES`] messages and [`MAX_BODY_BYTES`]
    /// bytes with a `[truncated ...]` trailer naming how many were dropped.
    #[must_use]
    pub fn render_body(&self) -> String {
        let mut body = String::new();
        let mut shown = 0usize;
        for message in &self.messages {
            let time = chrono::DateTime::from_timestamp(message.timestamp, 0)
                .map_or_else(|| "??:??:??".to_owned(), |dt| dt.format("%H:%M:%S").to_string());
            let line = format!(
                "{time} [{}] {}\n",
                priority_label(message.priority),
                message.text
            );
            if shown >= MAX_MESSAGES || body.len() + line.len() > MAX_BODY_BYTES {
                break;
            }
            body.push_str(&line);
            shown += 1;
        }
        let total = self.messages.len();
        if shown < total {
            use std::fmt::Write as _;
            // Writing to a String is infallible; no panic path.
            let _ = writeln!(body, "[truncated: {} more messages]", total - shown);
        }
        body
    }

    /// Project to a [`Document`]: the capped day log is embedded, its sha256 is
    /// the `content_hash` (the reconcile key), and the flat metadata carries
    /// every filter tag (`host`, `unit`, `timestamp`).
    ///
    /// # Errors
    /// Returns [`Error::Metadata`](crate::Error::Metadata) if the tag object
    /// exceeds the store's size or key limits.
    pub fn into_document(self) -> Result<Document> {
        let external_id = self.external_id();
        let body = self.render_body();
        let content_hash = source_meta::hash_body(body.as_bytes());
        let title = format!("{} warnings/errors {}", self.unit, self.date);
        // Recency axis: the day's last message, so "what broke recently"
        // ranks a still-failing unit's latest day first.
        let timestamp = self.messages.last().map(|message| message.timestamp);

        let mut meta = Map::new();
        meta.insert(keys::SOURCE.to_owned(), json!(SOURCE_TAG));
        meta.insert(keys::EXTERNAL_ID.to_owned(), json!(external_id));
        meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
        meta.insert(keys::TITLE.to_owned(), json!(title));
        meta.insert(keys::HOST.to_owned(), json!(self.host));
        meta.insert(keys::UNIT.to_owned(), json!(self.unit));
        if let Some(timestamp) = timestamp {
            meta.insert(keys::TIMESTAMP.to_owned(), json!(timestamp));
        }
        let meta_json = Value::Object(meta);

        source_meta::check_metadata(&external_id, &meta_json).context(MetadataSnafu {
            external_id: external_id.clone(),
        })?;

        Ok(Document {
            external_id,
            file_name: format!("{}-{}.txt", self.unit, self.date),
            mime: "text/plain",
            body: body.into_bytes(),
            meta_json,
            content_hash,
        })
    }
}
