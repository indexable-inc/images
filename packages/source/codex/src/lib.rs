//! Adapter turning Codex CLI history — the flat prompt log and the full
//! session rollouts — into embeddable, tagged [`source_meta`] documents for
//! the multi-source `search` store.
//!
//! # Grain
//! Two record shapes, both per-item:
//!
//! - One [`Document`] per submitted **prompt**. Codex stores history as a
//!   flat append log of `{session_id, ts, text}` lines (default
//!   `~/.codex/history.jsonl`), so a record is one user prompt.
//!   `external_id = "codex:{session_id}:{ts}:{content_hash}"`, so the id is
//!   stable under history compaction (it is content-derived, not
//!   positional): an append-only log re-ingests only its new prompts, and
//!   the content-hash reconcile in `search-core` skips everything already
//!   uploaded.
//! - One [`Document`] per session-rollout **item** (a message, or a tool
//!   call with its output folded in, like the claude adapter). Codex writes
//!   the full record of each session — assistant turns, tool calls and
//!   outputs — as JSONL under `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`.
//!   `external_id = "codex:{session_id}:{content_hash}"`: a resumed session
//!   replays its source session's items into a new file with fresh
//!   timestamps, so the id is content-derived per session and the replayed
//!   copies dedupe instead of duplicating. Rollout bodies pass through the
//!   shared [`source_meta::sanitize`] pipeline (ANSI stripped, credential
//!   shapes redacted, blobs collapsed, tool sections capped) before hashing.
//!
//! A prompt also appears in its session's rollout, as a `user` message: the
//! prompt log document is the durable copy (the log outlives session
//! cleanup), the rollout document the in-conversation one.
//!
//! # Tags
//! Every document's flat metadata carries the common header (`source`,
//! `external_id`, `content_hash`, `title`, `timestamp`) plus the
//! agent-history filter tags (`host`, `user`, `session_id`, and for rollout
//! items `role`, `record_type`, `model`, `cwd`/`project`, `tool_name`), so a
//! query can scope to a machine, user, session, or role.

#![forbid(unsafe_code)]

mod error;
mod record;
mod rollout;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use snafu::ResultExt as _;
use source_meta::{Document, Source, SourceAdapter};

pub use crate::error::Error;
use crate::error::{HostNameSnafu, ParseLineSnafu, ReadFileSnafu, Result};
pub use crate::record::{Entry, RolloutItem};

/// The `source` tag every Codex document carries.
pub const SOURCE_TAG: &str = "codex";

/// One raw line of `~/.codex/history.jsonl`.
#[derive(Debug, Deserialize)]
struct RawEntry {
    session_id: String,
    // A missing `ts` deserializes to `None`: serde treats `Option` fields as
    // implicitly optional.
    ts: Option<i64>,
    text: String,
}

/// A set of parsed Codex prompts and session-rollout items ready to project
/// into documents.
///
/// Construct with [`CodexHistory::open`], which reads the flat history log
/// (default [`default_path`]) and every rollout under the sessions directory
/// (default [`default_sessions_dir`]). Parsing happens up front so
/// [`SourceAdapter::documents`] is cheap to start.
#[derive(Debug)]
#[must_use]
pub struct CodexHistory {
    entries: Vec<Entry>,
    rollout_items: Vec<RolloutItem>,
}

impl CodexHistory {
    /// Parse the history log at `history` and every session rollout under
    /// `sessions` (either may be absent), tagging each record with an
    /// explicit `host` and `user`. The fleet's multi-user pass uses this so
    /// it can tag per-account; [`open`](Self::open) resolves them
    /// automatically.
    ///
    /// A missing sessions directory yields no items; a rollout file that
    /// cannot be read or parsed is logged and skipped, not fatal — one
    /// corrupt rollout must not drop every other session for this account.
    /// The history log keeps its stricter contract: it is one file the
    /// caller asked for by name.
    ///
    /// # Errors
    /// Returns an error if the history file cannot be read or a line of it is
    /// not valid JSON, or if a sessions directory cannot be listed.
    pub fn open_with(
        history: Option<&Path>,
        sessions: Option<&Path>,
        host: &str,
        user: &str,
    ) -> Result<Self> {
        let mut entries = Vec::new();
        if let Some(path) = history {
            parse_history(path, host, user, &mut entries)?;
        }

        let mut rollout_items = Vec::new();
        if let Some(dir) = sessions {
            let mut files = Vec::new();
            rollout::collect_rollouts(dir, &mut files)?;
            for file in files {
                match rollout::parse(&file, host, user) {
                    Ok(items) => rollout_items.extend(items),
                    Err(error) => {
                        eprintln!("[codex] skipping rollout {}: {error}", file.display());
                    }
                }
            }
        }
        Ok(Self {
            entries,
            rollout_items,
        })
    }

    /// Parse the history log and session rollouts, resolving `host` (via
    /// `gethostname`) and `user` (the OS user) automatically. This is the
    /// entry point the `indexer`'s `--codex-file`/`--codex-sessions` (and
    /// `--local`) uses.
    ///
    /// # Errors
    /// Returns an error if the host name cannot be resolved, or the sources
    /// cannot be read or parsed (see [`open_with`](Self::open_with)).
    pub fn open(history: Option<&Path>, sessions: Option<&Path>) -> Result<Self> {
        let host = hostname()?;
        let user = os_user();
        Self::open_with(history, sessions, &host, &user)
    }

    /// Number of parsed records (prompts plus rollout items).
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len() + self.rollout_items.len()
    }

    /// Whether no records were parsed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.rollout_items.is_empty()
    }
}

/// Parse the flat prompt log at `path` into `entries`.
fn parse_history(path: &Path, host: &str, user: &str, entries: &mut Vec<Entry>) -> Result<()> {
    let contents = std::fs::read_to_string(path).context(ReadFileSnafu {
        path: path.to_path_buf(),
    })?;
    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let raw: RawEntry = serde_json::from_str(line).context(ParseLineSnafu {
            path: path.to_path_buf(),
            line: index + 1,
        })?;
        entries.push(Entry {
            host: host.to_owned(),
            user: user.to_owned(),
            session_id: raw.session_id,
            timestamp: raw.ts,
            text: raw.text,
        });
    }
    Ok(())
}

impl SourceAdapter for CodexHistory {
    type Error = Error;

    fn source(&self) -> Source {
        Source::new(SOURCE_TAG)
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Clone into an owned iterator so the result is `'static + Send`,
        // independent of `&self` (mirrors the claude/slack adapters). The
        // dedup filter keeps the first document per `external_id`: a resumed
        // session replays its source session's items into a second rollout
        // file under the same content-derived ids (see
        // `RolloutItem::external_id`), and a batch must not carry the same id
        // twice.
        let mut seen = HashSet::new();
        self.entries
            .clone()
            .into_iter()
            .map(Entry::into_document)
            .chain(
                self.rollout_items
                    .clone()
                    .into_iter()
                    .map(RolloutItem::into_document),
            )
            .filter(move |result| {
                result
                    .as_ref()
                    .map_or(true, |document| seen.insert(document.external_id.clone()))
            })
    }
}

/// The default Codex history log: `~/.codex/history.jsonl`, or `None` when
/// `HOME` is unset.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex").join("history.jsonl"))
}

/// The default Codex session-rollout directory: `~/.codex/sessions`, or
/// `None` when `HOME` is unset.
#[must_use]
pub fn default_sessions_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex").join("sessions"))
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
