//! The typed per-command record and its projection to a search [`Document`].

use source_meta::{Document, keys};
use serde_json::{Map, Value, json};
use snafu::ResultExt as _;

use crate::SOURCE_TAG;
use crate::error::{MetadataSnafu, Result};

/// One recorded shell command from atuin, with its filter tags.
#[derive(Debug, Clone)]
pub struct Entry {
    /// atuin's stable per-command id.
    pub id: String,
    /// The command line to embed.
    pub command: String,
    /// Working directory the command ran in, when recorded.
    pub cwd: Option<String>,
    /// Short hostname the command ran on.
    pub host: String,
    /// OS user that ran the command, when derivable.
    pub user: Option<String>,
    /// atuin session id grouping commands from one shell session.
    pub session: Option<String>,
    /// Process exit status, when recorded.
    pub exit: Option<i64>,
    /// Command time as epoch seconds, when recorded.
    pub timestamp: Option<i64>,
}

impl Entry {
    /// Stable store id: `atuin:{id}`, reusing atuin's own unique command id so a
    /// re-sync only uploads commands not already stored.
    #[must_use]
    pub fn external_id(&self) -> String {
        format!("atuin:{}", self.id)
    }

    /// Project to a [`Document`]: the command is embedded, its sha256 is the
    /// `content_hash` (the reconcile key), and the flat metadata carries every
    /// filter tag.
    ///
    /// # Errors
    /// Returns [`Error::Metadata`](crate::Error::Metadata) if the tag object
    /// exceeds the store's size or key limits.
    pub fn into_document(self) -> Result<Document> {
        let external_id = self.external_id();
        let content_hash = source_meta::hash_body(self.command.as_bytes());
        let title = title_for(&self.command);

        let mut meta = Map::new();
        meta.insert(keys::SOURCE.to_owned(), json!(SOURCE_TAG));
        meta.insert("external_id".to_owned(), json!(external_id));
        meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
        meta.insert(keys::TITLE.to_owned(), json!(title));
        meta.insert(keys::HOST.to_owned(), json!(self.host));
        insert_some(&mut meta, keys::USER, self.user.map(Value::from));
        insert_some(&mut meta, keys::CWD, self.cwd.map(Value::from));
        insert_some(&mut meta, keys::SESSION_ID, self.session.map(Value::from));
        insert_some(&mut meta, keys::EXIT_STATUS, self.exit.map(Value::from));
        insert_some(&mut meta, keys::TIMESTAMP, self.timestamp.map(Value::from));
        let meta_json = Value::Object(meta);

        source_meta::check_metadata(&external_id, &meta_json)
            .context(MetadataSnafu { external_id: external_id.clone() })?;

        Ok(Document {
            external_id,
            file_name: format!("{}.txt", self.id),
            mime: "text/plain",
            body: self.command.into_bytes(),
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

/// Build a short human label from the first non-empty command line (capped).
fn title_for(command: &str) -> String {
    let snippet: String = command
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(80)
        .collect();
    format!("shell: {snippet}")
}
