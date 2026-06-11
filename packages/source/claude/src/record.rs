//! The typed per-message record and its projection to a search [`Document`].

use serde_json::{Map, Value, json};
use snafu::ResultExt as _;
use source_meta::{Document, keys};

use crate::error::{MetadataSnafu, Result};

/// The constant tags shared by every message in one transcript file: where it
/// was recorded and the file's fallback project/session identity. A line's own
/// `cwd`/`sessionId` override the fallbacks.
#[derive(Debug, Clone)]
pub struct MessageOrigin {
    /// Short hostname the transcript was recorded on.
    pub host: String,
    /// OS user that owns the transcript.
    pub user: String,
    /// Fallback project slug (the transcript file's parent directory name).
    pub project: String,
    /// Fallback session id (the transcript file stem).
    pub session_id: String,
}

/// One embeddable transcript message: the rendered body plus its filter tags.
#[derive(Debug, Clone)]
pub struct Message {
    /// Short hostname the transcript was recorded on.
    pub host: String,
    /// OS user that owns the transcript.
    pub user: String,
    /// Project the session ran in (the message `cwd`, or the file fallback).
    pub project: String,
    /// Claude Code session id.
    pub session_id: String,
    /// Stable per-message uuid assigned by Claude Code.
    pub uuid: String,
    /// Parent message uuid, threading the conversation.
    pub parent_uuid: Option<String>,
    /// Message role (`user`/`assistant`/`system`).
    pub role: String,
    /// Transcript record type (the line's `type` field).
    pub record_type: String,
    /// Model id, for an assistant message.
    pub model: Option<String>,
    /// Working directory the session ran in.
    pub cwd: Option<String>,
    /// Git branch checked out during the message, when recorded.
    pub git_branch: Option<String>,
    /// Tool name, for a message whose content invokes a tool.
    pub tool_name: Option<String>,
    /// Assistant input token count, when recorded.
    pub input_tokens: Option<i64>,
    /// Assistant output token count, when recorded.
    pub output_tokens: Option<i64>,
    /// Message time as epoch seconds, when the line carried a timestamp.
    pub timestamp: Option<i64>,
    /// Rendered text to embed.
    pub body: String,
}

impl Message {
    /// Stable store id: `claude:{session_id}:{uuid}`. Per message, so an
    /// append-only transcript only ever uploads its new messages.
    #[must_use]
    pub fn external_id(&self) -> String {
        format!("claude:{}:{}", self.session_id, self.uuid)
    }

    /// Project to a [`Document`]: the rendered body is embedded, its sha256 is
    /// the `content_hash` (the reconcile key), and the flat metadata carries
    /// every filter tag.
    ///
    /// # Errors
    /// Returns [`Error::Metadata`](crate::Error::Metadata) if the tag object
    /// exceeds the store's size or key limits.
    pub fn into_document(self) -> Result<Document> {
        let external_id = self.external_id();
        let content_hash = source_meta::hash_body(self.body.as_bytes());
        let title = title_for(&self.role, &self.project, &self.body);

        let mut meta = Map::new();
        meta.insert(keys::SOURCE.to_owned(), json!("claude_history"));
        meta.insert("external_id".to_owned(), json!(external_id));
        meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
        meta.insert(keys::TITLE.to_owned(), json!(title));
        meta.insert(keys::HOST.to_owned(), json!(self.host));
        meta.insert(keys::USER.to_owned(), json!(self.user));
        meta.insert(keys::PROJECT.to_owned(), json!(self.project));
        meta.insert(keys::SESSION_ID.to_owned(), json!(self.session_id));
        meta.insert(keys::MESSAGE_UUID.to_owned(), json!(self.uuid));
        meta.insert(keys::ROLE.to_owned(), json!(self.role));
        meta.insert(keys::RECORD_TYPE.to_owned(), json!(self.record_type));
        insert_some(
            &mut meta,
            keys::PARENT_UUID,
            self.parent_uuid.map(Value::from),
        );
        insert_some(&mut meta, keys::MODEL, self.model.map(Value::from));
        insert_some(&mut meta, keys::CWD, self.cwd.map(Value::from));
        insert_some(
            &mut meta,
            keys::GIT_BRANCH,
            self.git_branch.map(Value::from),
        );
        insert_some(&mut meta, keys::TOOL_NAME, self.tool_name.map(Value::from));
        insert_some(
            &mut meta,
            keys::INPUT_TOKENS,
            self.input_tokens.map(Value::from),
        );
        insert_some(
            &mut meta,
            keys::OUTPUT_TOKENS,
            self.output_tokens.map(Value::from),
        );
        insert_some(&mut meta, keys::TIMESTAMP, self.timestamp.map(Value::from));
        let meta_json = Value::Object(meta);

        source_meta::check_metadata(&external_id, &meta_json).context(MetadataSnafu {
            external_id: external_id.clone(),
        })?;

        Ok(Document {
            external_id,
            file_name: format!("{}.txt", self.uuid),
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

/// Build a short human label: role, project, and the first non-empty body line
/// (capped), so a hit lists readably without dumping the whole message.
fn title_for(role: &str, project: &str, body: &str) -> String {
    let snippet: String = body
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(80)
        .collect();
    format!("{role} @ {project}: {snippet}")
}
