//! Adapter turning a Linear issue export into embeddable [`search_meta`]
//! documents.
//!
//! A Linear export is a directory of JSON written by the team's exporter:
//! `metadata.json` (organization, team key/name, counts) and `issues.json` (an
//! array of issue objects, description and comments inline). This crate reads
//! that directory and projects each issue into one [`search_meta::Document`]:
//! the embedded body is a human-readable rendering of the issue (title, status
//! line, description, and every comment, oldest first), and the flat metadata is
//! the common [`search_meta`] envelope merged with Linear-specific filter keys
//! (`identifier`, `team_key`, `state_type`, labels, `has_pr`, ...).
//!
//! Grain is one document per issue. Comments in the export are newest-first, so
//! they are sorted ascending by `createdAt` before rendering, which keeps the
//! body (and therefore its `content_hash`) stable for an unchanged issue.
//!
//! The crate is pure: it reads two files in [`LinearExport::open`] and otherwise
//! does no I/O. It depends only on [`search_meta`], serde, snafu, and chrono.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use chrono::DateTime;
use search_meta::{Document, Source, SourceAdapter, keys};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use snafu::{ResultExt as _, Snafu};

/// All failures surfaced by this crate.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// An export file (`metadata.json` or `issues.json`) could not be read.
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
    #[snafu(display("metadata limit exceeded for issue {external_id}"))]
    Metadata {
        /// The record whose metadata overflowed.
        external_id: String,
        /// Underlying limit error.
        source: search_meta::MetadataError,
    },
}

/// Convenient result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A parsed Linear export directory, ready to project into documents.
///
/// Construct with [`LinearExport::open`]; the heavy work (parsing
/// `issues.json`) happens there, so [`SourceAdapter::documents`] only renders
/// already-parsed issues.
#[derive(Debug)]
pub struct LinearExport {
    team_key: String,
    issues: Vec<Issue>,
}

impl LinearExport {
    /// Open an export directory: read `metadata.json` for the team key and
    /// eagerly parse `issues.json` into owned issues.
    ///
    /// Linear is small (low thousands of issues), so parsing eagerly keeps
    /// [`SourceAdapter::documents`] cheap and infallible to start iterating.
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

        let issues_path = dir.join("issues.json");
        let issues_bytes = std::fs::read(&issues_path).context(ReadFileSnafu {
            path: issues_path.clone(),
        })?;
        let issues: Vec<Issue> =
            serde_json::from_slice(&issues_bytes).context(ParseJsonSnafu { path: issues_path })?;

        Ok(Self {
            team_key: metadata.team.key,
            issues,
        })
    }

    /// The team key (e.g. `ENG`) read from `metadata.json`.
    #[must_use]
    pub fn team_key(&self) -> &str {
        &self.team_key
    }

    /// Number of issues parsed from the export.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.issues.len()
    }

    /// Whether the export contained no issues.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.issues.is_empty()
    }
}

impl SourceAdapter for LinearExport {
    type Error = Error;

    fn source(&self) -> Source {
        Source::Linear
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Clone the parsed issues into an owned `Vec` so the returned iterator
        // is `'static` and `Send`, independent of `&self`'s lifetime.
        let team_key = self.team_key.clone();
        self.issues
            .clone()
            .into_iter()
            .map(move |issue| issue.into_document(&team_key))
    }
}

// ---------------------------------------------------------------------------
// Export schema (serde mirror of the JSON the exporter writes).
// ---------------------------------------------------------------------------

/// `metadata.json` shape; only the team block is consumed.
#[derive(Debug, Deserialize)]
struct ExportMetadata {
    team: TeamMeta,
}

/// The `team` block of `metadata.json`.
#[derive(Debug, Deserialize)]
struct TeamMeta {
    key: String,
}

/// One issue object from `issues.json`. Optional JSON fields use
/// `#[serde(default)]` since this is an external schema, not runtime config.
/// The exporter emits `camelCase` keys, mirrored here to `snake_case` fields.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Issue {
    id: String,
    identifier: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    priority: i64,
    #[serde(default)]
    priority_label: Option<String>,
    #[serde(default)]
    estimate: Option<f64>,
    url: String,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    completed_at: Option<String>,
    #[serde(default)]
    canceled_at: Option<String>,
    #[serde(default)]
    archived_at: Option<String>,
    state: State,
    #[serde(default)]
    assignee: Option<User>,
    #[serde(default)]
    creator: Option<User>,
    #[serde(default)]
    labels: NodeList<Label>,
    #[serde(default)]
    project: Option<Project>,
    #[serde(default)]
    project_milestone: Option<Milestone>,
    #[serde(default)]
    cycle: Option<Cycle>,
    #[serde(default)]
    parent: Option<Parent>,
    #[serde(default)]
    attachments: NodeList<Attachment>,
    #[serde(default)]
    comments: NodeList<Comment>,
}

/// A Linear workflow state. `type` is the stable filter axis; `name` is display.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    name: String,
    #[serde(rename = "type")]
    state_type: String,
}

/// A Linear user (assignee, creator, or comment author).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/// A label on an issue.
#[derive(Debug, Clone, Deserialize)]
struct Label {
    name: String,
}

/// The project an issue belongs to.
#[derive(Debug, Clone, Deserialize)]
struct Project {
    name: String,
}

/// A project milestone.
#[derive(Debug, Clone, Deserialize)]
struct Milestone {
    name: String,
}

/// A cycle an issue is in.
#[derive(Debug, Clone, Deserialize)]
struct Cycle {
    number: i64,
}

/// The parent issue, when this is a sub-issue.
#[derive(Debug, Clone, Deserialize)]
struct Parent {
    identifier: String,
}

/// An attachment (link) on an issue.
#[derive(Debug, Clone, Deserialize)]
struct Attachment {
    #[serde(default)]
    title: Option<String>,
    url: String,
}

/// A comment on an issue.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comment {
    #[serde(default)]
    body: Option<String>,
    created_at: String,
    #[serde(default)]
    user: Option<User>,
}

/// Linear's relay-style `{ "nodes": [...] }` wrapper.
#[derive(Debug, Clone, Deserialize)]
struct NodeList<T> {
    #[serde(default = "Vec::new")]
    nodes: Vec<T>,
}

// A manual `Default` (rather than `#[derive(Default)]`) so an empty `NodeList`
// needs no `T: Default` bound: an absent `labels`/`comments`/`attachments`
// field deserializes to an empty list whatever the element type is.
impl<T> Default for NodeList<T> {
    fn default() -> Self {
        Self { nodes: Vec::new() }
    }
}

// ---------------------------------------------------------------------------
// Projection: issue -> Document.
// ---------------------------------------------------------------------------

impl Issue {
    /// Render this issue into a [`Document`]: build the body, the flat metadata,
    /// validate the metadata against the store limits, then assemble.
    fn into_document(self, team_key: &str) -> Result<Document> {
        let external_id = format!("linear:issue:{}", self.id);
        let body = self.render_body(team_key).into_bytes();
        let content_hash = search_meta::hash_body(&body);
        let title = format!("{}: {}", self.identifier, self.title);

        let timestamp = parse_epoch_seconds(&self.updated_at);
        let meta_json = self.build_meta(&external_id, team_key, &content_hash, &title, timestamp);

        search_meta::check_metadata(&external_id, &meta_json).context(MetadataSnafu {
            external_id: external_id.clone(),
        })?;

        Ok(Document {
            external_id,
            file_name: format!("{}.txt", self.identifier),
            mime: "text/plain",
            body,
            meta_json,
            content_hash,
        })
    }

    /// Build the flat metadata object: common envelope + Linear extras.
    fn build_meta(
        &self,
        external_id: &str,
        team_key: &str,
        content_hash: &str,
        title: &str,
        timestamp: Option<i64>,
    ) -> Value {
        let mut meta = Map::new();

        // Common envelope.
        meta.insert(keys::SOURCE.to_owned(), json!(Source::Linear.as_str()));
        meta.insert("external_id".to_owned(), json!(external_id));
        meta.insert(keys::CONTENT_HASH.to_owned(), json!(content_hash));
        meta.insert(keys::TITLE.to_owned(), json!(title));
        meta.insert("url".to_owned(), json!(self.url));
        if let Some(ts) = timestamp {
            meta.insert(keys::TIMESTAMP.to_owned(), json!(ts));
        }

        // Linear extras.
        meta.insert(keys::IDENTIFIER.to_owned(), json!(self.identifier));
        meta.insert(keys::TEAM_KEY.to_owned(), json!(team_key));
        meta.insert(keys::STATE_TYPE.to_owned(), json!(self.state.state_type));
        meta.insert("state_name".to_owned(), json!(self.state.name));
        meta.insert("priority".to_owned(), json!(self.priority));

        if let Some(email) = self.assignee.as_ref().and_then(|u| u.email.as_deref()) {
            meta.insert(keys::ASSIGNEE_EMAIL.to_owned(), json!(email));
        }

        let labels: Vec<&str> = self.labels.nodes.iter().map(|l| l.name.as_str()).collect();
        meta.insert(keys::LABELS.to_owned(), json!(labels));

        if let Some(project) = self.project.as_ref() {
            meta.insert("project_name".to_owned(), json!(project.name));
        }
        if let Some(cycle) = self.cycle.as_ref() {
            meta.insert("cycle_number".to_owned(), json!(cycle.number));
        }
        if let Some(parent) = self.parent.as_ref() {
            meta.insert("parent_identifier".to_owned(), json!(parent.identifier));
        }

        let is_archived = self.archived_at.is_some();
        meta.insert(keys::IS_ARCHIVED.to_owned(), json!(is_archived));

        let pr_urls: Vec<&str> = self
            .attachments
            .nodes
            .iter()
            .map(|a| a.url.as_str())
            .filter(|url| url.contains("/pull/"))
            .collect();
        meta.insert("has_pr".to_owned(), json!(!pr_urls.is_empty()));
        meta.insert("pr_urls".to_owned(), json!(pr_urls));

        Value::Object(meta)
    }

    /// Render the human-readable body that gets embedded.
    fn render_body(&self, team_key: &str) -> String {
        // `write!`/`writeln!` into a `String` is infallible, so the returned
        // `fmt::Result`s are deliberately ignored; this avoids the
        // `format_push_string` lint that `push_str(&format!(..))` would trip.
        use std::fmt::Write as _;

        let mut out = String::new();

        // Header line.
        let _ = writeln!(out, "{}: {}", self.identifier, self.title);

        // Status line.
        let priority_label = self.priority_label.as_deref().unwrap_or("No priority");
        let assignee = self
            .assignee
            .as_ref()
            .and_then(display_user)
            .unwrap_or_else(|| "unassigned".to_owned());
        // Rust's float `Display` prints whole estimates without a trailing
        // `.0` (`3.0` -> "3") and keeps fractional ones (`1.5` -> "1.5").
        let estimate = self
            .estimate
            .map_or_else(|| "-".to_owned(), |value| value.to_string());
        let _ = writeln!(
            out,
            "State: {} ({}) | Priority: {priority_label} | Assignee: {assignee} | Estimate: {estimate}",
            self.state.name, self.state.state_type,
        );

        // Context line.
        let project = self.project.as_ref().map_or("-", |p| p.name.as_str());
        let milestone = self
            .project_milestone
            .as_ref()
            .map_or("-", |m| m.name.as_str());
        let cycle = self
            .cycle
            .as_ref()
            .map_or_else(|| "-".to_owned(), |c| c.number.to_string());
        let parent = self.parent.as_ref().map_or("-", |p| p.identifier.as_str());
        let _ = writeln!(
            out,
            "Team: {team_key} | Project: {project} | Milestone: {milestone} | Cycle: {cycle} | Parent: {parent}",
        );

        // Labels line.
        let labels: Vec<&str> = self.labels.nodes.iter().map(|l| l.name.as_str()).collect();
        let _ = writeln!(out, "Labels: {}", labels.join(", "));

        // Provenance line.
        let creator = self
            .creator
            .as_ref()
            .and_then(display_user)
            .unwrap_or_else(|| "unknown".to_owned());
        let _ = write!(
            out,
            "Created by {creator} on {} | Updated {}",
            self.created_at, self.updated_at,
        );
        if let Some(ts) = self.completed_at.as_deref() {
            let _ = write!(out, " | Completed {ts}");
        }
        if let Some(ts) = self.canceled_at.as_deref() {
            let _ = write!(out, " | Canceled {ts}");
        }
        if let Some(ts) = self.archived_at.as_deref() {
            let _ = write!(out, " | Archived {ts}");
        }
        out.push('\n');

        // Links line.
        let links: Vec<String> = self
            .attachments
            .nodes
            .iter()
            .map(|a| {
                let label = a.title.as_deref().unwrap_or("link");
                format!("{label} -> {}", a.url)
            })
            .collect();
        let _ = writeln!(out, "Links: {}", links.join(", "));

        // Description block. The leading blank line separates it from the
        // header; `writeln!` keeps each `write!` string free of a trailing
        // newline (which would trip `write_with_newline`).
        let description = self
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or("(no description)");
        out.push('\n');
        let _ = writeln!(out, "Description:");
        let _ = writeln!(out, "{description}");

        // Comments block, sorted ascending by createdAt (export is newest-first).
        let mut comments: Vec<&Comment> = self.comments.nodes.iter().collect();
        comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        out.push('\n');
        let _ = writeln!(out, "Comments ({}):", comments.len());
        for comment in comments {
            let author = comment
                .user
                .as_ref()
                .and_then(|u| u.display_name.clone())
                .unwrap_or_else(|| "unknown".to_owned());
            let comment_body = comment.body.as_deref().unwrap_or("");
            let _ = writeln!(out, "[{author} @ {}]", comment.created_at);
            let _ = writeln!(out, "{comment_body}");
            let _ = writeln!(out, "---");
        }

        out
    }
}

/// A user's best display name: prefer `name`, then `displayName`, then email.
fn display_user(user: &User) -> Option<String> {
    user.name
        .clone()
        .or_else(|| user.display_name.clone())
        .or_else(|| user.email.clone())
}

/// Parse an RFC3339 timestamp into epoch seconds, or `None` if it does not parse.
fn parse_epoch_seconds(timestamp: &str) -> Option<i64> {
    let parsed = DateTime::parse_from_rfc3339(timestamp).ok()?;
    Some(parsed.timestamp())
}
