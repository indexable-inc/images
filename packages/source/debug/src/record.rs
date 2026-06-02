//! The typed per-session debug record and its projection to a search [`Document`].

use source_meta::{Document, keys};
use serde_json::{Map, Value, json};
use snafu::ResultExt as _;

use crate::SOURCE_TAG;
use crate::error::{MetadataSnafu, Result};

/// One Claude Code debug log (a whole `~/.claude/debug/<session>.txt` file) with
/// its filter tags.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Session id, taken from the debug file's stem.
    pub session_id: String,
    /// Full debug log text to embed.
    pub body: String,
    /// Host the session ran on.
    pub host: String,
    /// OS user that owns the session.
    pub user: String,
    /// File mtime as epoch seconds, the recency axis (debug lines are timestamped
    /// internally, but the file mtime is a dependency-free stand-in).
    pub timestamp: Option<i64>,
}

impl Entry {
    /// Stable store id: `claude_debug:{session_id}`, reusing the session id so a
    /// re-sync only re-uploads logs whose bytes changed (the content hash gates
    /// it), and a still-growing session's log re-uploads until the session ends.
    #[must_use]
    pub fn external_id(&self) -> String {
        format!("claude_debug:{}", self.session_id)
    }

    /// Project to a [`Document`]: the debug text is embedded, its sha256 is the
    /// `content_hash` (the reconcile key), and the flat metadata carries every
    /// filter tag.
    ///
    /// # Errors
    /// Returns [`Error::Metadata`](crate::Error::Metadata) if the tag object
    /// exceeds the store's size or key limits.
    pub fn into_document(self) -> Result<Document> {
        let external_id = self.external_id();
        let content_hash = source_meta::hash_body(self.body.as_bytes());
        let title = format!("debug: {}", self.session_id);

        let mut meta = Map::new();
        meta.insert(keys::SOURCE.to_owned(), json!(SOURCE_TAG));
        meta.insert("external_id".to_owned(), json!(external_id));
        meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
        meta.insert(keys::TITLE.to_owned(), json!(title));
        meta.insert(keys::HOST.to_owned(), json!(self.host));
        meta.insert(keys::USER.to_owned(), json!(self.user));
        meta.insert(keys::SESSION_ID.to_owned(), json!(self.session_id));
        insert_some(&mut meta, keys::TIMESTAMP, self.timestamp.map(Value::from));
        let meta_json = Value::Object(meta);

        source_meta::check_metadata(&external_id, &meta_json)
            .context(MetadataSnafu { external_id: external_id.clone() })?;

        Ok(Document {
            external_id,
            file_name: format!("{}.txt", self.session_id),
            mime: "text/plain",
            body: self.body.into_bytes(),
            meta_json,
            content_hash,
        })
    }
}

/// Insert `key` only when a value is present, keeping absent tags off the record
/// rather than serializing nulls.
fn insert_some(meta: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        meta.insert(key.to_owned(), value);
    }
}
