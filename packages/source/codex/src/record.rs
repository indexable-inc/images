//! The typed per-prompt record and its projection to a search [`Document`].

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

/// Build a short human label from the first non-empty prompt line (capped), so a
/// hit lists readably without dumping the whole prompt.
fn title_for(text: &str) -> String {
    let snippet: String = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(80)
        .collect();
    format!("codex: {snippet}")
}
