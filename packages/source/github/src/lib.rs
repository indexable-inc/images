//! Adapter turning a GitHub export into embeddable [`source_meta`] documents.
//!
//! A GitHub export is a directory of JSON written by the bundled `export.sh`
//! (which drives the `gh` CLI): `metadata.json` (export provenance and the set
//! of repos covered) and `items.json` (a single combined array of issues and
//! pull requests). Each array element is self-describing: it carries its own
//! `repo` (`owner/name`) and a `kind` discriminator, so one export can span many
//! repositories. The export script does the joins the GitHub API splits across
//! endpoints, nesting each PR's reviews and inline review threads under the PR
//! item, so this crate stays a pure reader with no join logic.
//!
//! This crate projects each item into one [`source_meta::Document`]: the
//! embedded body is a human-readable rendering (title, status line, body, then
//! every comment, review, and review thread oldest-first) and the flat metadata
//! is the common [`source_meta`] envelope merged with GitHub-specific filter
//! keys (`repo`, `number`, `state`, `is_pr`, labels, ...).
//!
//! Grain is one document per issue and one per pull request. The `external_id`
//! is `github:<owner>/<repo>:<number>` (a `:` separator, not `#`: the sink's
//! delete path puts the id in a URL path, where a `#` would be parsed as a
//! fragment and silently truncate the id, like `git:<repo>:<sha>`), stable
//! across re-exports, so the sink
//! reconciles in place: an edited item re-embeds and an unchanged one is skipped
//! (`sync_documents` keys on `external_id` + `content_hash`). The indexer pass
//! uploads and updates only; it does not delete items dropped from a later
//! export, so a closed or removed item keeps its last-exported version
//! searchable until a separate garbage-collection pass runs.
//!
//! The crate is pure: it reads two files in [`GithubExport::open`] and otherwise
//! does no I/O. It depends only on [`source_meta`], serde, snafu, and chrono.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use snafu::{ResultExt as _, Snafu};
use source_meta::{Document, Source, SourceAdapter, keys};

/// All failures surfaced by this crate.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// An export file (`metadata.json` or `items.json`) could not be read.
    #[snafu(display("failed to read export file {}", path.display()))]
    ReadFile {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },

    /// An export file did not parse as the expected JSON shape.
    #[snafu(display("failed to parse export file {}", path.display()))]
    ParseJson {
        /// File that failed to parse.
        path: PathBuf,
        /// Underlying serde error.
        source: serde_json::Error,
    },

    /// A built document's metadata exceeded the store's size or key limits.
    #[snafu(display("metadata limit exceeded for item {external_id}"))]
    Metadata {
        /// The record whose metadata overflowed.
        external_id: String,
        /// Underlying limit error.
        source: source_meta::MetadataError,
    },
}

/// Convenient result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A parsed GitHub export directory, ready to project into documents.
///
/// Construct with [`GithubExport::open`]; the heavy work (parsing `items.json`)
/// happens there, so [`SourceAdapter::documents`] only renders already-parsed
/// items.
#[derive(Debug)]
pub struct GithubExport {
    repos: Vec<String>,
    items: Vec<Item>,
}

impl GithubExport {
    /// Open an export directory: read `metadata.json` for provenance and eagerly
    /// parse `items.json` into owned items.
    ///
    /// A GitHub export is export-driven and repo-scale (low tens of thousands of
    /// items), so parsing eagerly keeps [`SourceAdapter::documents`] cheap and
    /// infallible to start iterating.
    ///
    /// # Errors
    /// Returns [`Error::ReadFile`] if either file cannot be read and
    /// [`Error::ParseJson`] if either does not match the expected JSON shape.
    pub fn open(dir: &Path) -> Result<Self> {
        let metadata_path = dir.join("metadata.json");
        let metadata_bytes = std::fs::read(&metadata_path).context(ReadFileSnafu {
            path: metadata_path.clone(),
        })?;
        let metadata: ExportMetadata =
            serde_json::from_slice(&metadata_bytes).context(ParseJsonSnafu {
                path: metadata_path,
            })?;

        let items_path = dir.join("items.json");
        let items_bytes = std::fs::read(&items_path).context(ReadFileSnafu {
            path: items_path.clone(),
        })?;
        let items: Vec<Item> =
            serde_json::from_slice(&items_bytes).context(ParseJsonSnafu { path: items_path })?;

        Ok(Self {
            repos: metadata.repos,
            items,
        })
    }

    /// The repositories (`owner/name`) this export covers, per `metadata.json`.
    #[must_use]
    pub fn repos(&self) -> &[String] {
        &self.repos
    }

    /// Number of items (issues + pull requests) parsed from the export.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the export contained no items.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl SourceAdapter for GithubExport {
    type Error = Error;

    fn source(&self) -> Source {
        Source::new("github")
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Clone the parsed items into an owned `Vec` so the returned iterator is
        // `'static` and `Send`, independent of `&self`'s lifetime.
        self.items.clone().into_iter().map(Item::into_document)
    }
}

// ---------------------------------------------------------------------------
// Export schema (serde mirror of the normalized JSON `export.sh` writes).
// ---------------------------------------------------------------------------

/// `metadata.json` shape: export provenance and the repos covered.
#[derive(Debug, Deserialize)]
struct ExportMetadata {
    #[serde(default)]
    repos: Vec<String>,
}

/// Whether an item is an issue or a pull request. The export script tags every
/// item so this crate never has to infer type from the GitHub number space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Issue,
    Pr,
}

/// One item from `items.json`. Optional JSON fields use `#[serde(default)]`
/// since this is an external schema, not runtime config; pull-request-only
/// fields are simply absent or empty on issues.
#[derive(Debug, Clone, Deserialize)]
struct Item {
    kind: Kind,
    repo: String,
    number: i64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    assignees: Vec<String>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    closed_at: Option<String>,
    url: String,
    #[serde(default)]
    comments: Vec<Comment>,

    // Pull-request-only fields.
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    is_draft: bool,
    #[serde(default)]
    base_ref: Option<String>,
    #[serde(default)]
    head_ref: Option<String>,
    #[serde(default)]
    reviews: Vec<Review>,
    #[serde(default)]
    review_threads: Vec<ReviewThread>,
}

/// A top-level comment on an issue or pull request.
#[derive(Debug, Clone, Deserialize)]
struct Comment {
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    body: Option<String>,
    created_at: String,
}

/// A pull-request review (the review-level summary, not the inline comments).
#[derive(Debug, Clone, Deserialize)]
struct Review {
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    state: Option<String>,
    submitted_at: String,
}

/// An inline review thread on a pull request: a diff location plus its comments.
#[derive(Debug, Clone, Deserialize)]
struct ReviewThread {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<i64>,
    #[serde(default)]
    comments: Vec<ThreadComment>,
}

/// One comment inside an inline review thread.
#[derive(Debug, Clone, Deserialize)]
struct ThreadComment {
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    body: Option<String>,
    created_at: String,
}

// ---------------------------------------------------------------------------
// Projection: item -> Document.
// ---------------------------------------------------------------------------

impl Item {
    /// Render this item into a [`Document`]: build the body, the flat metadata,
    /// validate the metadata against the store limits, then assemble.
    fn into_document(self) -> Result<Document> {
        // `:` not `#`: the sink deletes by putting this id in a URL path, where a
        // `#` is parsed as a fragment and truncates the id. Mirrors `git:repo:sha`.
        let external_id = format!("github:{}:{}", self.repo, self.number);
        let body = self.render_body().into_bytes();
        let content_hash = source_meta::hash_body(&body);
        let title = format!("{}#{}: {}", self.repo, self.number, self.title);

        let timestamp = parse_epoch_seconds(&self.updated_at);
        let meta_json = self.build_meta(&external_id, &content_hash, &title, timestamp);

        source_meta::check_metadata(&external_id, &meta_json).context(MetadataSnafu {
            external_id: external_id.clone(),
        })?;

        Ok(Document {
            external_id,
            file_name: format!("{}-{}.txt", self.repo.replace('/', "_"), self.number),
            mime: "text/plain",
            body,
            meta_json,
            content_hash,
        })
    }

    /// Whether this item is a pull request.
    const fn is_pr(&self) -> bool {
        matches!(self.kind, Kind::Pr)
    }

    /// Build the flat metadata object: common envelope + GitHub extras.
    fn build_meta(
        &self,
        external_id: &str,
        content_hash: &str,
        title: &str,
        timestamp: Option<i64>,
    ) -> Value {
        let mut meta = Map::new();

        // Common envelope.
        meta.insert(keys::SOURCE.to_owned(), json!(Source::new("github").as_str()));
        meta.insert("external_id".to_owned(), json!(external_id));
        meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
        meta.insert(keys::TITLE.to_owned(), json!(title));
        meta.insert("url".to_owned(), json!(self.url));
        if let Some(ts) = timestamp {
            meta.insert(keys::TIMESTAMP.to_owned(), json!(ts));
        }

        // GitHub extras.
        meta.insert(keys::REPO.to_owned(), json!(self.repo));
        meta.insert(keys::NUMBER.to_owned(), json!(self.number));
        meta.insert(keys::STATE.to_owned(), json!(self.state.to_lowercase()));
        meta.insert(keys::IS_PR.to_owned(), json!(self.is_pr()));

        if let Some(author) = self.author.as_deref() {
            meta.insert(keys::AUTHOR_NAME.to_owned(), json!(author));
        }

        let labels: Vec<&str> = self.labels.iter().map(String::as_str).collect();
        meta.insert(keys::LABELS.to_owned(), json!(labels));

        if !self.assignees.is_empty() {
            let assignees: Vec<&str> = self.assignees.iter().map(String::as_str).collect();
            meta.insert("assignees".to_owned(), json!(assignees));
        }

        if self.is_pr() {
            meta.insert("is_draft".to_owned(), json!(self.is_draft));
        }

        Value::Object(meta)
    }

    /// Render the human-readable body that gets embedded.
    ///
    /// Every nested list (comments, reviews, review threads) is sorted by its
    /// timestamp before rendering so an unchanged item produces identical bytes
    /// (and therefore a stable `content_hash`) regardless of export order.
    fn render_body(&self) -> String {
        // `write!`/`writeln!` into a `String` is infallible, so the returned
        // `fmt::Result`s are deliberately ignored; this avoids the
        // `format_push_string` lint that `push_str(&format!(..))` would trip.
        use std::fmt::Write as _;

        let mut out = String::new();
        let kind = if self.is_pr() { "PR" } else { "issue" };

        // Header line.
        let _ = writeln!(out, "{}#{} [{kind}] {}", self.repo, self.number, self.title);

        // Status line.
        let mut status = format!("State: {}", self.state.to_lowercase());
        if self.is_pr() && self.is_draft {
            status.push_str(" (draft)");
        }
        if let Some(ts) = self.merged_at.as_deref() {
            let _ = write!(status, " | Merged {ts}");
        }
        if let Some(ts) = self.closed_at.as_deref() {
            let _ = write!(status, " | Closed {ts}");
        }
        let _ = writeln!(out, "{status}");

        // Provenance line.
        let author = self.author.as_deref().unwrap_or("unknown");
        let _ = writeln!(
            out,
            "Author: {author} | Created {} | Updated {}",
            self.created_at, self.updated_at,
        );

        // Branch line (PRs only).
        if self.is_pr() {
            let base = self.base_ref.as_deref().unwrap_or("?");
            let head = self.head_ref.as_deref().unwrap_or("?");
            let _ = writeln!(out, "Branches: {head} -> {base}");
        }

        // Labels and assignees.
        let _ = writeln!(out, "Labels: {}", self.labels.join(", "));
        let _ = writeln!(out, "Assignees: {}", self.assignees.join(", "));
        let _ = writeln!(out, "URL: {}", self.url);

        // Body block.
        let body = self
            .body
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .unwrap_or("(no body)");
        out.push('\n');
        let _ = writeln!(out, "Body:");
        let _ = writeln!(out, "{body}");

        // Comments block, oldest-first.
        let mut comments: Vec<&Comment> = self.comments.iter().collect();
        comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        out.push('\n');
        let _ = writeln!(out, "Comments ({}):", comments.len());
        for comment in comments {
            let author = comment.author.as_deref().unwrap_or("unknown");
            let body = comment.body.as_deref().unwrap_or("");
            let _ = writeln!(out, "[{author} @ {}]", comment.created_at);
            let _ = writeln!(out, "{body}");
            let _ = writeln!(out, "---");
        }

        if self.is_pr() {
            // Reviews block, oldest-first.
            let mut reviews: Vec<&Review> = self.reviews.iter().collect();
            reviews.sort_by(|a, b| a.submitted_at.cmp(&b.submitted_at));
            out.push('\n');
            let _ = writeln!(out, "Reviews ({}):", reviews.len());
            for review in reviews {
                let author = review.author.as_deref().unwrap_or("unknown");
                let state = review.state.as_deref().unwrap_or("COMMENTED");
                let body = review.body.as_deref().unwrap_or("");
                let _ = writeln!(out, "[{author} @ {}] {state}", review.submitted_at);
                let _ = writeln!(out, "{body}");
                let _ = writeln!(out, "---");
            }

            // Review threads block. Threads are ordered by (path, line, earliest
            // comment) and each thread's comments oldest-first, so the rendering
            // is deterministic regardless of export order.
            let mut threads: Vec<&ReviewThread> = self.review_threads.iter().collect();
            threads.sort_by(|a, b| {
                let key = |t: &ReviewThread| {
                    (
                        t.path.clone().unwrap_or_default(),
                        t.line.unwrap_or(i64::MAX),
                        t.comments
                            .iter()
                            .map(|c| c.created_at.clone())
                            .min()
                            .unwrap_or_default(),
                    )
                };
                key(a).cmp(&key(b))
            });
            out.push('\n');
            let _ = writeln!(out, "Review threads ({}):", threads.len());
            for thread in threads {
                let path = thread.path.as_deref().unwrap_or("(unknown path)");
                let line = thread.line.map_or_else(|| "?".to_owned(), |l| l.to_string());
                let _ = writeln!(out, "[{path}:{line}]");
                let mut comments: Vec<&ThreadComment> = thread.comments.iter().collect();
                comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
                for comment in comments {
                    let author = comment.author.as_deref().unwrap_or("unknown");
                    let body = comment.body.as_deref().unwrap_or("");
                    let _ = writeln!(out, "  [{author} @ {}]", comment.created_at);
                    let _ = writeln!(out, "  {body}");
                }
                let _ = writeln!(out, "---");
            }
        }

        out
    }
}

/// Parse an RFC3339 timestamp into epoch seconds, or `None` if it does not parse.
fn parse_epoch_seconds(timestamp: &str) -> Option<i64> {
    let parsed = DateTime::parse_from_rfc3339(timestamp).ok()?;
    Some(parsed.timestamp())
}
