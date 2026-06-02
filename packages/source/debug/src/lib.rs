//! Adapter turning Claude Code debug logs into embeddable, tagged
//! [`source_meta`] documents for the multi-source `search` store.
//!
//! # Grain
//! One [`Document`] per session debug file. Claude Code writes one
//! `~/.claude/debug/<session-id>.txt` per session when run with `--debug`, a
//! line-oriented `TIMESTAMP [LEVEL] message` log of API/MCP/init/timing events.
//! `external_id = "claude_debug:{session_id}"`, so a re-sync re-uploads a log
//! only while its bytes keep changing (a live session grows its file); an ended
//! session's log is stable and skipped by the content-hash gate.
//!
//! # Tags
//! Every document's flat metadata carries the common header (`source`,
//! `external_id`, `content_hash`, `title`, `timestamp`) plus `host`, `user`, and
//! `session_id`, so a query can scope debug logs to a machine, user, or session.
//!
//! # Security
//! The privileged fleet run reads other accounts' debug logs as root. Like the
//! claude/codex adapters, the caller resolves the debug dir with a symlink-safe
//! path check, and this adapter additionally indexes only **regular** files: a
//! planted symlink in the debug dir (e.g. `<id>.txt -> /etc/shadow`) is reported
//! by `read_dir` as a symlink, not a file, so it is skipped rather than followed.

#![forbid(unsafe_code)]

mod error;
mod record;

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use source_meta::{Document, Source, SourceAdapter};
use snafu::ResultExt as _;

pub use crate::error::Error;
pub use crate::record::Entry;
use crate::error::{ReadDirSnafu, ReadFileSnafu, Result};

/// The `source` tag every debug-log document carries.
pub const SOURCE_TAG: &str = "claude_debug";

/// A set of Claude Code debug logs ready to project into documents.
///
/// Construct with [`DebugLogs::open_with`]. A missing debug dir is normal (the
/// session was not run with `--debug`) and yields an empty set, not an error.
#[derive(Debug)]
#[must_use]
pub struct DebugLogs {
    entries: Vec<Entry>,
}

impl DebugLogs {
    /// Read every regular `*.txt` debug log under `dir`, tagging each with
    /// `host` and `user`. The fleet sync binary uses this so it can tag logs for
    /// the account whose home it is reading, not the process owner.
    ///
    /// # Errors
    /// Returns an error if the directory or a log file cannot be read. A missing
    /// directory is not an error: it returns an empty set.
    pub fn open_with(dir: &Path, host: &str, user: &str) -> Result<Self> {
        let mut entries = Vec::new();
        let read = match std::fs::read_dir(dir) {
            Ok(read) => read,
            // No `--debug` runs here: nothing to index, not a failure.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self { entries });
            }
            Err(error) => return Err(error).context(ReadDirSnafu { path: dir.to_path_buf() }),
        };

        for entry in read {
            let entry = entry.context(ReadDirSnafu { path: dir.to_path_buf() })?;
            let file_type = entry.file_type().context(ReadDirSnafu { path: dir.to_path_buf() })?;
            // Regular files only. `read_dir` reports the `latest` symlink (and any
            // planted symlink) as a symlink, so `is_file()` is false and it is
            // skipped without ever being opened or followed.
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("txt") {
                continue;
            }
            let Some(session_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if session_id.is_empty() {
                continue;
            }
            let body = std::fs::read_to_string(&path).context(ReadFileSnafu { path: path.clone() })?;
            if body.trim().is_empty() {
                continue;
            }
            let timestamp = entry.metadata().ok().as_ref().and_then(mtime_secs);
            entries.push(Entry {
                session_id: session_id.to_owned(),
                body,
                host: host.to_owned(),
                user: user.to_owned(),
                timestamp,
            });
        }
        Ok(Self { entries })
    }

    /// Number of parsed debug logs.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no debug logs were parsed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The default debug dir: `~/.claude/debug`, or `None` when `HOME` is unset.
    #[must_use]
    pub fn default_dir() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude/debug"))
    }
}

impl SourceAdapter for DebugLogs {
    type Error = Error;

    fn source(&self) -> Source {
        Source::new(SOURCE_TAG)
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Clone into an owned iterator so the result is `'static + Send`,
        // independent of `&self` (mirrors the other source adapters).
        self.entries.clone().into_iter().map(Entry::into_document)
    }
}

/// A file's mtime as epoch seconds, when it is representable.
fn mtime_secs(meta: &std::fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    let secs = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}
