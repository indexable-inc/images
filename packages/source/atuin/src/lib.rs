//! Adapter turning atuin shell history into embeddable, tagged
//! [`source_meta`] documents for the multi-source `search` store.
//!
//! # Grain
//! One [`Document`] per recorded **command**. atuin keeps one sqlite history db
//! (default `~/.local/share/atuin/history.db`) capturing commands from every
//! shell it wraps (nushell, zsh, bash), so they share one `shell` corpus rather
//! than a per-shell source. `external_id = "atuin:{id}"` reuses atuin's own
//! stable command id, so a re-sync only uploads commands the store is missing.
//!
//! # Tags
//! Every document's flat metadata carries the common header (`source`,
//! `external_id`, `content_hash`, `title`, `timestamp`) plus the shell filter
//! tags (`host`, `user`, `cwd`, `session_id`, `exit_status`), so a query can
//! scope to a machine, user, directory, session, or success/failure.

#![forbid(unsafe_code)]

mod error;
mod record;

use std::path::{Path, PathBuf};

use source_meta::{Document, Source, SourceAdapter};
use rusqlite::{Connection, OpenFlags};
use snafu::ResultExt as _;

pub use crate::error::Error;
pub use crate::record::Entry;
use crate::error::{OpenDbSnafu, QuerySnafu, Result};

/// The `source` tag every atuin command document carries. atuin records
/// commands from every shell (nushell, zsh, bash), so one `shell` corpus covers
/// them rather than a per-shell tag.
pub const SOURCE_TAG: &str = "shell";

/// atuin stores timestamps as nanoseconds since the Unix epoch; the common
/// `timestamp` tag is epoch seconds.
const NANOS_PER_SEC: i64 = 1_000_000_000;

/// A set of atuin history commands ready to project into documents.
///
/// Construct with [`AtuinHistory::open`], which reads the sqlite db read-only
/// (so a live shell writing to it is never blocked). Parsing happens up front so
/// [`SourceAdapter::documents`] is cheap to start.
#[derive(Debug)]
#[must_use]
pub struct AtuinHistory {
    entries: Vec<Entry>,
}

impl AtuinHistory {
    /// Open the atuin history db at `path` read-only and read every
    /// non-deleted command.
    ///
    /// Opened as `immutable=1` via a `SQLite` URI, not just
    /// `SQLITE_OPEN_READ_ONLY`: atuin runs in WAL mode, and a plain read-only
    /// open still tries to touch the `-wal`/`-shm` sidecars and a lock file. When
    /// a live shell holds the db or the home dir is not writable by this process
    /// (the privileged fleet run reads other accounts' homes), that fails with
    /// `SQLITE_CANTOPEN` (code 14). `immutable=1` tells `SQLite` the file cannot
    /// change underneath it, so it skips all sidecar and locking I/O and reads
    /// the main db file directly. The trade-off is a possibly-stale view if a
    /// writer is mid-commit, which is fine for a periodic indexer (the next run
    /// catches up).
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened or queried.
    pub fn open(path: &Path) -> Result<Self> {
        // URI form so `immutable=1` applies; the path is the trusted db path the
        // caller resolved (no untrusted query-string injection).
        let uri = format!("file:{}?immutable=1", path.display());
        let conn = Connection::open_with_flags(
            &uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
        .context(OpenDbSnafu { path: path.to_path_buf() })?;
        let entries = read_entries(&conn)?;
        Ok(Self { entries })
    }

    /// Number of parsed commands.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no commands were parsed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The default atuin history db: `~/.local/share/atuin/history.db`, or
    /// `None` when `HOME` is unset.
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/atuin/history.db"))
    }
}

impl SourceAdapter for AtuinHistory {
    type Error = Error;

    fn source(&self) -> Source {
        Source::new(SOURCE_TAG)
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Clone into an owned iterator so the result is `'static + Send`,
        // independent of `&self` (mirrors the claude/codex/slack adapters).
        self.entries.clone().into_iter().map(Entry::into_document)
    }
}

/// Read every non-deleted, non-empty command from the atuin `history` table.
fn read_entries(conn: &Connection) -> Result<Vec<Entry>> {
    let mut stmt = conn
        .prepare(
            "select id, timestamp, exit, command, cwd, session, hostname \
             from history where deleted_at is null order by timestamp",
        )
        .context(QuerySnafu)?;
    let rows = stmt
        .query_map([], |row| {
            let timestamp_ns: Option<i64> = row.get(1)?;
            let hostname: Option<String> = row.get(6)?;
            let (host, user) = split_host_user(hostname.as_deref());
            Ok(Entry {
                id: row.get(0)?,
                command: row.get(3)?,
                cwd: non_empty(row.get(4)?),
                host,
                user,
                session: non_empty(row.get(5)?),
                exit: row.get(2)?,
                timestamp: timestamp_ns.map(|ns| ns / NANOS_PER_SEC),
            })
        })
        .context(QuerySnafu)?;

    let mut entries = Vec::new();
    for row in rows {
        let entry = row.context(QuerySnafu)?;
        if entry.command.trim().is_empty() {
            continue;
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// atuin records `hostname` as `"<host>:<user>"`. Split on the first colon; fall
/// back to the whole value as the host with no user.
fn split_host_user(hostname: Option<&str>) -> (String, Option<String>) {
    let Some(value) = hostname else {
        return ("unknown".to_owned(), None);
    };
    value.split_once(':').map_or_else(
        || (value.to_owned(), None),
        |(host, user)| (host.to_owned(), non_empty(Some(user.to_owned()))),
    )
}

/// Treat an absent or empty string column as no value.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}
