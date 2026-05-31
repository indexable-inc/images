//! Adapter turning Claude Code agent transcripts into embeddable, tagged
//! [`search_meta`] documents for the multi-source `search` store.
//!
//! # Grain
//! One [`Document`] per transcript **message** (a `user`/`assistant` line that
//! carries content). `external_id = "claude:{session_id}:{uuid}"`, so an
//! append-only transcript re-ingests only its new messages: the content-hash
//! reconcile in `search-core` skips everything already uploaded.
//!
//! # Tags
//! Every document's flat metadata carries the common header (`source`,
//! `external_id`, `content_hash`, `title`, `timestamp`) plus the agent-history
//! filter tags (`host`, `user`, `project`, `session_id`, `message_uuid`,
//! `parent_uuid`, `role`, `record_type`, `model`, `cwd`, `git_branch`,
//! `tool_name`, token counts), so a query can scope to a machine, user,
//! project, session, or role.

#![forbid(unsafe_code)]

mod error;
mod record;
mod transcript;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use search_meta::{Document, Source, SourceAdapter};
use snafu::ResultExt as _;

pub use crate::error::Error;
use crate::error::{HostNameSnafu, ListDirSnafu, Result};
use crate::record::{Message, MessageOrigin};

/// The `source` tag every Claude transcript document carries.
pub const SOURCE_TAG: &str = "claude_history";

/// A set of parsed Claude transcript messages ready to project into documents.
///
/// Construct with [`ClaudeHistoryExport::open`], which recursively reads every
/// `*.jsonl` transcript under a directory (e.g. `~/.claude/projects`). Parsing
/// happens up front so [`SourceAdapter::documents`] is cheap to start.
#[derive(Debug)]
#[must_use]
pub struct ClaudeHistoryExport {
    messages: Vec<Message>,
}

impl ClaudeHistoryExport {
    /// Open and parse every transcript under `dir`, tagging each message with an
    /// explicit `host` and `user`. The fleet sync binary uses this so it can tag
    /// per-machine; [`open`](Self::open) resolves them automatically.
    ///
    /// # Errors
    /// Returns an error if a directory cannot be listed, a transcript cannot be
    /// read, or a line is not valid JSON.
    pub fn open_with(dir: &Path, host: &str, user: &str) -> Result<Self> {
        let mut files = Vec::new();
        let mut visited = HashSet::new();
        collect_transcripts(dir, &mut files, &mut visited)?;

        let mut messages = Vec::new();
        for file in files {
            let origin = origin_for(&file, host, user);
            messages.extend(transcript::parse(&file, &origin)?);
        }
        Ok(Self { messages })
    }

    /// Open every transcript under `dir`, resolving `host` (via `gethostname`)
    /// and `user` (the OS user) automatically. This is the entry point
    /// `search ingest --source claude_history <dir>` uses.
    ///
    /// # Errors
    /// Returns an error if the host name cannot be resolved, or a transcript
    /// cannot be read or parsed.
    pub fn open(dir: &Path) -> Result<Self> {
        let host = hostname()?;
        let user = os_user();
        Self::open_with(dir, &host, &user)
    }

    /// Number of parsed messages.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether no messages were parsed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

impl SourceAdapter for ClaudeHistoryExport {
    type Error = Error;

    fn source(&self) -> Source {
        Source::new(SOURCE_TAG)
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Clone into an owned iterator so the result is `'static + Send`,
        // independent of `&self` (mirrors the slack/linear adapters).
        self.messages.clone().into_iter().map(Message::into_document)
    }
}

/// Recursively collect `*.jsonl` transcript files under `dir`.
///
/// Symlinks are followed because Claude's history directory is itself a symlink
/// in some setups (`~/.claude/projects` points at the real store). `visited`
/// holds the canonical path of every directory already entered, so a symlink
/// cycle is broken instead of recursing forever. This is for the intended
/// unprivileged, single-user run, reading only the invoking user's own files; a
/// privileged or multi-user shipper must additionally reject symlinks with
/// `O_NOFOLLOW` to avoid the confused-deputy class (see ix `history-ship`'s
/// symlink finding).
fn collect_transcripts(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    visited: &mut HashSet<PathBuf>,
) -> Result<()> {
    let canonical = std::fs::canonicalize(dir).context(ListDirSnafu { path: dir.to_path_buf() })?;
    if !visited.insert(canonical) {
        // Already walked this real directory (a symlink pointed back into the
        // tree); stop rather than loop.
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).context(ListDirSnafu { path: dir.to_path_buf() })?;
    for entry in entries {
        let entry = entry.context(ListDirSnafu { path: dir.to_path_buf() })?;
        let path = entry.path();
        let metadata = std::fs::metadata(&path).context(ListDirSnafu { path: path.clone() })?;
        if metadata.is_dir() {
            collect_transcripts(&path, out, visited)?;
        } else if metadata.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

/// Derive a file's fallback identity tags: project from the parent directory
/// name, session from the file stem. A line's own `cwd`/`sessionId` override
/// these when present.
fn origin_for(file: &Path, host: &str, user: &str) -> MessageOrigin {
    let project = file
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let session_id = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    MessageOrigin {
        host: host.to_owned(),
        user: user.to_owned(),
        project,
        session_id,
    }
}

/// Resolve the host name for record tagging.
fn hostname() -> Result<String> {
    let raw = nix::unistd::gethostname()
        .map_err(std::io::Error::from)
        .context(HostNameSnafu)?;
    Ok(raw.to_string_lossy().into_owned())
}

/// The OS user owning the process, for the `user` tag.
fn os_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}
