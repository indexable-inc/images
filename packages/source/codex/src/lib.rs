//! Adapter turning Codex CLI prompt history into embeddable, tagged
//! [`source_meta`] documents for the multi-source `search` store.
//!
//! # Grain
//! One [`Document`] per submitted **prompt**. Codex stores history as a flat
//! append log of `{session_id, ts, text}` lines (default `~/.codex/history.jsonl`),
//! so a record is one user prompt; there is no assistant side.
//! `external_id = "codex:{session_id}:{seq}"`, where `seq` is the prompt's
//! 0-based ordinal within its session in file order, so an append-only log
//! re-ingests only its new prompts: the content-hash reconcile in `search-core`
//! skips everything already uploaded.
//!
//! # Tags
//! Every document's flat metadata carries the common header (`source`,
//! `external_id`, `content_hash`, `title`, `timestamp`) plus the agent-history
//! filter tags (`host`, `user`, `session_id`), so a query can scope to a
//! machine, user, or session.

#![forbid(unsafe_code)]

mod error;
mod record;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use source_meta::{Document, Source, SourceAdapter};
use serde::Deserialize;
use snafu::ResultExt as _;

pub use crate::error::Error;
pub use crate::record::Entry;
use crate::error::{HostNameSnafu, ParseLineSnafu, ReadFileSnafu, Result};

/// The `source` tag every Codex prompt document carries.
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

/// A set of parsed Codex prompts ready to project into documents.
///
/// Construct with [`CodexHistory::open`], which reads the flat history log
/// (default [`default_path`]). Parsing happens up front so
/// [`SourceAdapter::documents`] is cheap to start.
#[derive(Debug)]
#[must_use]
pub struct CodexHistory {
    entries: Vec<Entry>,
}

impl CodexHistory {
    /// Parse the history log at `path`, tagging each prompt with an explicit
    /// `host` and `user`. The fleet sync binary uses this so it can tag
    /// per-machine; [`open`](Self::open) resolves them automatically.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read, or a line is not valid JSON.
    pub fn open_with(path: &Path, host: &str, user: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path).context(ReadFileSnafu { path: path.to_path_buf() })?;
        let mut seq_by_session: HashMap<String, usize> = HashMap::new();
        let mut entries = Vec::new();
        for (index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let raw: RawEntry = serde_json::from_str(line).context(ParseLineSnafu {
                path: path.to_path_buf(),
                line: index + 1,
            })?;
            let seq = seq_by_session.entry(raw.session_id.clone()).or_insert(0);
            entries.push(Entry {
                host: host.to_owned(),
                user: user.to_owned(),
                session_id: raw.session_id,
                seq: *seq,
                timestamp: raw.ts,
                text: raw.text,
            });
            *seq += 1;
        }
        Ok(Self { entries })
    }

    /// Parse the history log at `path`, resolving `host` (via `gethostname`) and
    /// `user` (the OS user) automatically. This is the entry point the
    /// `indexer`'s `--codex-file` (and `--local`) uses.
    ///
    /// # Errors
    /// Returns an error if the host name cannot be resolved, or the file cannot
    /// be read or parsed.
    pub fn open(path: &Path) -> Result<Self> {
        let host = hostname()?;
        let user = os_user();
        Self::open_with(path, &host, &user)
    }

    /// Number of parsed prompts.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no prompts were parsed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl SourceAdapter for CodexHistory {
    type Error = Error;

    fn source(&self) -> Source {
        Source::new(SOURCE_TAG)
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Clone into an owned iterator so the result is `'static + Send`,
        // independent of `&self` (mirrors the claude/slack adapters).
        self.entries.clone().into_iter().map(Entry::into_document)
    }
}

/// The default Codex history log: `~/.codex/history.jsonl`, or `None` when
/// `HOME` is unset.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex").join("history.jsonl"))
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
