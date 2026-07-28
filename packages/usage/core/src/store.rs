//! `SQLite` source of truth and the spool compactor.
//!
//! Exactly one process writes to `SQLite` at a time: [`compact`] holds an
//! exclusive flock for its whole run, so wrapped tools never contend on the
//! database (they only append to the spool). WAL keeps readers non-blocking.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

use anyhow::Context as _;
use rusqlite::{Connection, OptionalExtension as _};

/// Current `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 1;
/// Errors-table cap; the oldest rows beyond it are pruned at compaction.
const MAX_ERROR_ROWS: i64 = 500;
/// Live spool file name inside the state dir.
const SPOOL_NAME: &str = "usage.spool";
/// Prefix of spool files renamed aside for folding (crash leftovers included).
const COMPACTING_PREFIX: &str = "usage.spool.compacting-";

/// Open (creating and migrating as needed) the usage database at `path`.
///
/// # Errors
/// Directory creation or `SQLite` failures.
pub fn open(path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating state dir {}", parent.display()))?;
    }
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_millis(250))?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    conn.execute_batch(
        "BEGIN;
         CREATE TABLE IF NOT EXISTS counts(
           pkg TEXT NOT NULL,
           version TEXT NOT NULL,
           day TEXT NOT NULL,
           invocations INTEGER NOT NULL DEFAULT 0,
           nonzero_exits INTEGER NOT NULL DEFAULT 0,
           PRIMARY KEY (pkg, version, day));
         CREATE TABLE IF NOT EXISTS errors(
           id INTEGER PRIMARY KEY,
           ts_ms INTEGER NOT NULL,
           pkg TEXT NOT NULL,
           version TEXT NOT NULL,
           exit_code INTEGER,
           argv TEXT,
           cwd TEXT,
           duration_ms INTEGER,
           reported_ref TEXT);
         CREATE TABLE IF NOT EXISTS meta(
           key TEXT PRIMARY KEY,
           value TEXT NOT NULL);
         PRAGMA user_version = 1;
         COMMIT;",
    )
}

/// Read one meta value.
///
/// # Errors
/// `SQLite` failures.
pub fn meta_get(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
        row.get(0)
    })
    .optional()
}

/// Upsert one meta value.
///
/// # Errors
/// `SQLite` failures.
pub fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, value],
    )
    .map(|_| ())
}

/// Get, or mint and persist, the random per-install id.
///
/// # Errors
/// `SQLite` failures.
pub fn install_id(conn: &Connection) -> rusqlite::Result<String> {
    if let Some(id) = meta_get(conn, "install_id")? {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    meta_set(conn, "install_id", &id)?;
    Ok(id)
}

/// `YYYY-MM-DD` (UTC) for a Unix-milliseconds timestamp; `None` when the
/// timestamp is unrepresentable.
#[must_use]
pub fn day_from_ts_ms(ts_ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ts_ms).map(|ts| ts.format("%Y-%m-%d").to_string())
}

/// Outcome of a [`compact`] run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CompactStats {
    /// Spool records folded into the database.
    pub folded: u64,
    /// Lines skipped as torn or unparseable (a crash mid-append leaves at
    /// most one).
    pub skipped: u64,
}

/// Fold every pending spool file under `state_dir` into the database.
///
/// Holds an exclusive advisory lock (`compact.lock`) so this is the single
/// database writer, renames the live spool aside so hot-path writers start a
/// fresh one, then folds every `usage.spool.compacting-*` file (including
/// leftovers from crashed compactors) in one transaction each, deleting each
/// file after its transaction commits.
///
/// # Errors
/// Filesystem or `SQLite` failures. Safe to rerun: records fold at most once
/// because a file is deleted only after its transaction commits, and a
/// crash between commit and delete refolds counts for at most one file.
pub fn compact(state_dir: &Path) -> anyhow::Result<CompactStats> {
    std::fs::create_dir_all(state_dir)?;
    let lock = File::create(state_dir.join("compact.lock"))?;
    flock_exclusive(&lock)?;

    let live = state_dir.join(SPOOL_NAME);
    if live.exists() {
        let aside = state_dir.join(format!(
            "{COMPACTING_PREFIX}{}-{}",
            std::process::id(),
            crate::spool::now_ms()?
        ));
        std::fs::rename(&live, &aside)?;
    }

    let mut pending = Vec::new();
    for entry in std::fs::read_dir(state_dir)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(COMPACTING_PREFIX)
        {
            pending.push(entry.path());
        }
    }
    if pending.is_empty() {
        return Ok(CompactStats::default());
    }
    pending.sort();

    let mut conn = open(&state_dir.join("usage.db"))?;
    let mut stats = CompactStats::default();
    for file in pending {
        fold_file(&mut conn, &file, &mut stats)?;
        std::fs::remove_file(&file)?;
    }
    Ok(stats)
}

fn fold_file(conn: &mut Connection, file: &Path, stats: &mut CompactStats) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(file)?;
    let tx = conn.transaction()?;
    for line in text.lines() {
        match serde_json::from_str::<crate::spool::Record>(line) {
            Ok(record) => {
                if fold_record(&tx, &record)? {
                    stats.folded += 1;
                } else {
                    stats.skipped += 1;
                }
            }
            Err(_) => stats.skipped += 1,
        }
    }
    tx.execute(
        "DELETE FROM errors WHERE id NOT IN
           (SELECT id FROM errors ORDER BY id DESC LIMIT ?1)",
        [MAX_ERROR_ROWS],
    )?;
    tx.commit()?;
    Ok(())
}

/// Returns `false` (skip) only for records whose timestamp maps to no day.
fn fold_record(tx: &rusqlite::Transaction, record: &crate::spool::Record) -> anyhow::Result<bool> {
    let Some(day) = day_from_ts_ms(record.ts_ms) else {
        return Ok(false);
    };
    let failed = record.exit.is_some_and(|code| code != 0);
    tx.execute(
        "INSERT INTO counts(pkg, version, day, invocations, nonzero_exits)
         VALUES (?1, ?2, ?3, 1, ?4)
         ON CONFLICT(pkg, version, day) DO UPDATE SET
           invocations = invocations + 1,
           nonzero_exits = nonzero_exits + excluded.nonzero_exits",
        rusqlite::params![record.pkg, record.version, day, i64::from(failed)],
    )?;
    if failed {
        let argv_json = match &record.argv {
            Some(argv) => Some(serde_json::to_string(argv)?),
            None => None,
        };
        tx.execute(
            "INSERT INTO errors(ts_ms, pkg, version, exit_code, argv, cwd, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                record.ts_ms,
                record.pkg,
                record.version,
                record.exit,
                argv_json,
                record.cwd,
                record.duration_ms.map(i64::try_from).transpose()?,
            ],
        )?;
    }
    Ok(true)
}

fn flock_exclusive(file: &File) -> io::Result<()> {
    // SAFETY: flock(2) on a file descriptor `file` keeps open for the whole
    // compaction; the lock dies with the fd.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::{CompactStats, compact, day_from_ts_ms, install_id, open};
    use crate::spool::{Record, append_at};

    fn record(exit: Option<i32>) -> Record {
        Record {
            ts_ms: 1_700_000_000_000,
            pkg: "demo".to_owned(),
            version: "1.0".to_owned(),
            exit,
            duration_ms: Some(5),
            argv: exit.map(|_| vec!["demo".to_owned(), "--x".to_owned()]),
            cwd: None,
        }
    }

    #[test]
    fn epoch_is_day_zero() {
        assert_eq!(day_from_ts_ms(0).as_deref(), Some("1970-01-01"));
    }

    #[test]
    fn compacts_counts_errors_and_torn_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spool = dir.path().join("usage.spool");
        append_at(&spool, &record(Some(0))).expect("append ok record");
        append_at(&spool, &record(Some(3))).expect("append failing record");
        append_at(&spool, &record(None)).expect("append count-only record");
        // Simulate a crash mid-append: a torn, unterminated final line.
        let mut text = std::fs::read_to_string(&spool).expect("read spool");
        text.push_str("{\"ts_ms\":17");
        std::fs::write(&spool, text).expect("tear spool");

        let stats = compact(dir.path()).expect("compact");
        assert_eq!(
            stats,
            CompactStats {
                folded: 3,
                skipped: 1
            }
        );
        assert!(!spool.exists(), "live spool was renamed aside and removed");

        let conn = open(&dir.path().join("usage.db")).expect("open db");
        let row: (i64, i64) = conn
            .query_row(
                "SELECT invocations, nonzero_exits FROM counts
                 WHERE pkg = 'demo' AND version = '1.0' AND day = '2023-11-14'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("counts row");
        assert_eq!(row, (3, 1));
        let argv: String = conn
            .query_row("SELECT argv FROM errors", [], |row| row.get(0))
            .expect("error row");
        assert_eq!(argv, "[\"demo\",\"--x\"]");

        // Idempotent when nothing is pending.
        let stats = compact(dir.path()).expect("second compact");
        assert_eq!(stats, CompactStats::default());
    }

    #[test]
    fn install_id_is_stable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open(&dir.path().join("usage.db")).expect("open db");
        let first = install_id(&conn).expect("mint id");
        let second = install_id(&conn).expect("reread id");
        assert_eq!(first, second);
        assert_eq!(first.len(), 36, "uuid shape");
    }
}
