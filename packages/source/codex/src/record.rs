//! The typed per-prompt and per-rollout-item records and their projections
//! to a search [`Document`].

use serde_json::{Map, Value, json};
use snafu::ResultExt as _;
use source_meta::{Document, keys};

use crate::SOURCE_TAG;
use crate::error::{MetadataSnafu, Result};

/// One Codex prompt entry: the text the user submitted plus its filter tags.
///
/// Codex history is a flat append log of user prompts (`{session_id, ts,
/// text}`); there is no assistant side, so a record is one submitted prompt.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Short hostname the history was recorded on.
    pub host: String,
    /// OS user that owns the history.
    pub user: String,
    /// Codex session id grouping prompts from one run.
    pub session_id: String,
    /// Submission time as epoch seconds, when the line carried one.
    pub timestamp: Option<i64>,
    /// The submitted prompt text to embed.
    pub text: String,
}

impl Entry {
    /// Stable store id: `codex:{session_id}:{ts}:{content_hash}`.
    ///
    /// Deliberately NOT a file-position ordinal: Codex compacts its history log
    /// (dropping the oldest lines), which would shift any positional index and
    /// make surviving prompts look new — re-uploading them and orphaning the old
    /// ids. Keying on the session, timestamp, and content hash is invariant to
    /// compaction, so a prompt keeps one id for its lifetime.
    #[must_use]
    pub fn external_id(&self, content_hash: &str) -> String {
        let ts = self
            .timestamp
            .map_or_else(|| "na".to_owned(), |ts| ts.to_string());
        format!("codex:{}:{}:{}", self.session_id, ts, content_hash)
    }

    /// Project to a [`Document`]: the prompt text is embedded, its sha256 is the
    /// `content_hash` (the reconcile key), and the flat metadata carries every
    /// filter tag.
    ///
    /// # Errors
    /// Returns [`Error::Metadata`](crate::Error::Metadata) if the tag object
    /// exceeds the store's size or key limits.
    pub fn into_document(self) -> Result<Document> {
        let content_hash = source_meta::hash_body(self.text.as_bytes());
        let external_id = self.external_id(&content_hash);
        let title = title_for(&self.text);

        let mut meta = Map::new();
        meta.insert(keys::SOURCE.to_owned(), json!(SOURCE_TAG));
        meta.insert("external_id".to_owned(), json!(external_id));
        meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
        meta.insert(keys::TITLE.to_owned(), json!(title));
        meta.insert(keys::HOST.to_owned(), json!(self.host));
        meta.insert(keys::USER.to_owned(), json!(self.user));
        meta.insert(keys::SESSION_ID.to_owned(), json!(self.session_id));
        if let Some(timestamp) = self.timestamp {
            meta.insert(keys::TIMESTAMP.to_owned(), json!(timestamp));
        }
        let meta_json = Value::Object(meta);

        source_meta::check_metadata(&external_id, &meta_json).context(MetadataSnafu {
            external_id: external_id.clone(),
        })?;

        let file_name = format!("{external_id}.txt");
        Ok(Document {
            external_id,
            file_name,
            mime: "text/plain",
            body: self.text.into_bytes(),
            meta_json,
            content_hash,
        })
    }
}

/// One embeddable Codex session-rollout item: the rendered body plus its
/// filter tags.
///
/// Built by the `rollout` parser from `~/.codex/sessions/**/rollout-*.jsonl`;
/// covers the assistant side (messages, tool calls with their outputs folded
/// in) the flat prompt log does not record.
#[derive(Debug, Clone)]
pub struct RolloutItem {
    /// Short hostname the rollout was recorded on.
    pub host: String,
    /// OS user that owns the rollout.
    pub user: String,
    /// Codex session id (the nearest preceding `session_meta`).
    pub session_id: String,
    /// Item role (`user`/`assistant`, or `tool` for an orphan output).
    pub role: String,
    /// Rollout item kind (`message`, `function_call`, ...).
    pub record_type: String,
    /// Model id from the surrounding turn context, when recorded.
    pub model: Option<String>,
    /// Working directory the turn ran in, when recorded.
    pub cwd: Option<String>,
    /// Tool name, for a tool-call item.
    pub tool_name: Option<String>,
    /// Item time as epoch seconds, when the line carried a timestamp.
    pub timestamp: Option<i64>,
    /// Sanitized rendered text to embed.
    pub body: String,
}

impl RolloutItem {
    /// Stable store id: `codex:{session_id}:{content_hash}`.
    ///
    /// Deliberately NOT keyed on the line timestamp or file position: a
    /// resumed session replays its source session's items into a new rollout
    /// file with fresh timestamps and shifted positions, and either would
    /// re-key every replayed item into a duplicate. Keying on the session and
    /// the content hash makes the replay produce the *same* ids, which the
    /// adapter then dedupes. (The three-segment shape also cannot collide
    /// with a prompt's four-segment `codex:{session}:{ts}:{hash}`.) Identical
    /// repeats within one session collapse into one document, which is fine
    /// for search.
    #[must_use]
    pub fn external_id(&self, content_hash: &str) -> String {
        format!("codex:{}:{}", self.session_id, content_hash)
    }

    /// Project to a [`Document`]: the rendered body is embedded, its sha256
    /// is the `content_hash` (the reconcile key), and the flat metadata
    /// carries every filter tag.
    ///
    /// # Errors
    /// Returns [`Error::Metadata`](crate::Error::Metadata) if the tag object
    /// exceeds the store's size or key limits.
    pub fn into_document(self) -> Result<Document> {
        let content_hash = source_meta::hash_body(self.body.as_bytes());
        let external_id = self.external_id(&content_hash);
        let title = format!("codex {}: {}", self.role, snippet_of(&self.body));

        let mut meta = Map::new();
        meta.insert(keys::SOURCE.to_owned(), json!(SOURCE_TAG));
        meta.insert(keys::EXTERNAL_ID.to_owned(), json!(external_id));
        meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
        meta.insert(keys::TITLE.to_owned(), json!(title));
        meta.insert(keys::HOST.to_owned(), json!(self.host));
        meta.insert(keys::USER.to_owned(), json!(self.user));
        meta.insert(keys::SESSION_ID.to_owned(), json!(self.session_id));
        meta.insert(keys::ROLE.to_owned(), json!(self.role));
        meta.insert(keys::RECORD_TYPE.to_owned(), json!(self.record_type));
        insert_some(&mut meta, keys::MODEL, self.model.map(Value::from));
        // `project` mirrors the claude adapter, where it is the working
        // directory the session ran in; tagged under both keys so either
        // filter axis scopes codex and claude history alike.
        insert_some(&mut meta, keys::PROJECT, self.cwd.clone().map(Value::from));
        insert_some(&mut meta, keys::CWD, self.cwd.map(Value::from));
        insert_some(&mut meta, keys::TOOL_NAME, self.tool_name.map(Value::from));
        insert_some(&mut meta, keys::TIMESTAMP, self.timestamp.map(Value::from));
        let meta_json = Value::Object(meta);

        source_meta::check_metadata(&external_id, &meta_json).context(MetadataSnafu {
            external_id: external_id.clone(),
        })?;

        let file_name = format!("{external_id}.txt");
        Ok(Document {
            external_id,
            file_name,
            mime: "text/plain",
            body: self.body.into_bytes(),
            meta_json,
            content_hash,
        })
    }
}

/// Insert `key` only when a value is present, keeping absent tags off the
/// record rather than serializing nulls.
fn insert_some(meta: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        meta.insert(key.to_owned(), value);
    }
}

/// Build a short human label from the first non-empty prompt line (capped), so a
/// hit lists readably without dumping the whole prompt.
fn title_for(text: &str) -> String {
    format!("codex: {}", snippet_of(text))
}

/// The first non-empty line of a body, capped, for a title snippet.
fn snippet_of(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(80)
        .collect()
}
