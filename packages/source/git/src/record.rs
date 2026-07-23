//! The typed per-commit record and its projection to a search [`Document`].

use serde_json::{Map, Value, json};
use snafu::ResultExt as _;
use source_meta::{Document, keys};

use crate::SOURCE_TAG;
use crate::error::{MetadataSnafu, Result};

/// One git commit: the message to embed plus its filter tags.
#[derive(Debug, Clone)]
pub struct Commit {
    /// Repository slug (git remote, or directory name when there is no remote).
    pub repo: String,
    /// Full commit SHA.
    pub sha: String,
    /// Commit author name.
    pub author_name: String,
    /// Commit author email.
    pub author_email: String,
    /// Author time as epoch seconds.
    pub timestamp: i64,
    /// First line of the commit message.
    pub subject: String,
    /// Remaining commit message body (may be empty).
    pub body: String,
}

impl Commit {
    /// Stable store id: `git:{repo}:{sha}`. Per commit, so re-ingesting a repo
    /// only ever uploads its new commits.
    #[must_use]
    pub fn external_id(&self) -> String {
        format!("git:{}:{}", self.repo, self.sha)
    }

    /// Project to a [`Document`]: the commit message (subject + body) is
    /// embedded, its sha256 is the `content_hash`, and the flat metadata carries
    /// every filter tag. The diff is intentionally not embedded (too large and
    /// costly across full history); it lives in git, keyed by the `commit` tag.
    ///
    /// # Errors
    /// Returns [`Error::Metadata`](crate::Error::Metadata) if the tag object
    /// exceeds the store's size or key limits.
    pub fn into_document(self) -> Result<Document> {
        let external_id = self.external_id();
        let message = if self.body.trim().is_empty() {
            self.subject.clone()
        } else {
            format!("{}\n\n{}", self.subject, self.body)
        };
        let content_hash = source_meta::hash_body(message.as_bytes());
        let short = self.sha.get(..12).unwrap_or(&self.sha);
        let title = format!("{}@{}: {}", self.repo, short, self.subject);

        let mut meta = Map::new();
        meta.insert(keys::SOURCE.to_owned(), json!(SOURCE_TAG));
        meta.insert("external_id".to_owned(), json!(external_id));
        meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
        meta.insert(keys::TITLE.to_owned(), json!(title));
        meta.insert(keys::REPO.to_owned(), json!(self.repo));
        meta.insert(keys::COMMIT.to_owned(), json!(self.sha));
        meta.insert(keys::AUTHOR_NAME.to_owned(), json!(self.author_name));
        meta.insert(keys::AUTHOR_EMAIL.to_owned(), json!(self.author_email));
        meta.insert(keys::TIMESTAMP.to_owned(), json!(self.timestamp));
        let meta_json = Value::Object(meta);

        source_meta::check_metadata(&external_id, &meta_json).context(MetadataSnafu {
            external_id: external_id.clone(),
        })?;

        Ok(Document {
            external_id,
            file_name: format!("{}.txt", self.sha),
            mime: "text/plain",
            body: message.into_bytes(),
            meta_json,
            content_hash,
        })
    }
}
