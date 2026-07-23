//! Manifest persistence in `SQLite`.
//!
//! One shared database under the cache dir holds every checkout's entries,
//! keyed by `(root, rel_path)`. A shared DB (rather than one file per checkout)
//! centralizes concurrency control and turns a future cross-worktree
//! refcount-GC into a single `GROUP BY hash` query over the `files_by_hash`
//! index.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};
use snafu::{OptionExt as _, ResultExt as _};

use crate::content::ContentHash;
use crate::error::{CreateCacheDirSnafu, DbSnafu, NoCacheDirSnafu, OpenDbSnafu, Result};
use crate::manifest::{FileEntry, Manifest};

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS files (
    root     TEXT    NOT NULL,
    rel_path TEXT    NOT NULL,
    hash     TEXT    NOT NULL,
    mtime_ms INTEGER NOT NULL,
    size     INTEGER NOT NULL,
    PRIMARY KEY (root, rel_path)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS files_by_hash ON files (hash);
CREATE TABLE IF NOT EXISTS synced (
    root      TEXT NOT NULL,
    store     TEXT NOT NULL,
    signature TEXT NOT NULL,
    PRIMARY KEY (root, store)
) WITHOUT ROWID;";

/// Handle to the shared manifest database.
#[derive(Debug)]
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open the shared database at the default cache location, creating it (and
    /// its parent directory) if needed.
    ///
    /// # Errors
    /// Returns an error if no cache dir is available, the directory cannot be
    /// created, or the database cannot be opened or initialized.
    pub fn open() -> Result<Self> {
        Self::open_at(&db_path()?)
    }

    /// Open the shared database at an explicit path. Used by tests.
    ///
    /// # Errors
    /// Returns an error if the parent directory cannot be created or the
    /// database cannot be opened or initialized.
    pub fn open_at(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context(CreateCacheDirSnafu {
                path: parent.to_path_buf(),
            })?;
        }
        let conn = Connection::open(path).context(OpenDbSnafu {
            path: path.to_path_buf(),
        })?;

        // WAL + a busy timeout let several `search` processes share
        // this DB: concurrent readers plus one writer, writers serialized with
        // retry. auto_vacuum reclaims space in place after deletes.
        // synchronous=NORMAL is the right durability for a rebuildable cache
        // (safe across app crashes under WAL; only power loss can drop the last
        // commit, which a re-run reconstructs).
        //
        // Why SQLite, not LMDB: LMDB requires a preset map_size and its file
        // only ever grows -- deletes never reclaim space, so shrinking needs an
        // offline `mdb_copy -c` (http://www.lmdb.tech/doc/man1/mdb_copy_1.html,
        // https://www.lmdb.tech/doc/group__mdb__copy.html). For a churny
        // manifest that is exactly the wart to avoid, and SQLite's auto_vacuum
        // sidesteps it. The alternatives fail the multi-process requirement:
        // RocksDB and sled are single-process (sled's own docs point at LMDB
        // for multi-process), and redb documents only in-process MVCC. SQLite
        // and LMDB are the two with real multi-process support; SQLite wins on
        // in-place compaction and SQL-CLI inspectability.
        conn.execute_batch(
            "PRAGMA auto_vacuum=FULL;\
             PRAGMA journal_mode=WAL;\
             PRAGMA busy_timeout=5000;\
             PRAGMA synchronous=NORMAL;",
        )
        .context(DbSnafu)?;
        conn.execute_batch(SCHEMA).context(DbSnafu)?;

        Ok(Self { conn })
    }

    /// Load the stored manifest for `root`, or an empty manifest if none.
    ///
    /// # Errors
    /// Returns an error if the query fails or a row cannot be decoded.
    pub fn load(&self, root: &Path) -> Result<Manifest> {
        let root = root.to_string_lossy();
        let mut stmt = self
            .conn
            .prepare("SELECT rel_path, hash, mtime_ms, size FROM files WHERE root = ?1")
            .context(DbSnafu)?;
        let rows = stmt
            .query_map([root.as_ref()], |row| {
                let rel_path: String = row.get(0)?;
                let hash: String = row.get(1)?;
                // SQLite stores these as signed INTEGER; rusqlite 0.40 dropped
                // its `u64` column readers, so read `i64` and convert. A negative
                // value is impossible for an mtime/size, so reject it (surfacing
                // DB corruption) instead of wrapping it into a huge `u64`.
                let mtime_raw: i64 = row.get(2)?;
                let size_raw: i64 = row.get(3)?;
                let mtime_ms = u64::try_from(mtime_raw)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, mtime_raw))?;
                let size = u64::try_from(size_raw)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, size_raw))?;
                Ok(FileEntry {
                    rel_path,
                    hash: ContentHash::from_raw(hash),
                    mtime_ms,
                    size,
                })
            })
            .context(DbSnafu)?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row.context(DbSnafu)?);
        }
        Ok(Manifest { entries })
    }

    /// Replace the stored manifest for `root` with `manifest`, atomically.
    ///
    /// Stale rows (files no longer present in this checkout) are removed, so a
    /// sibling worktree's rows under a different root are never touched.
    ///
    /// # Errors
    /// Returns an error if the transaction or any statement fails.
    pub fn save(&mut self, root: &Path, manifest: &Manifest) -> Result<()> {
        let root = root.to_string_lossy();
        let tx = self.conn.transaction().context(DbSnafu)?;
        tx.execute("DELETE FROM files WHERE root = ?1", [root.as_ref()])
            .context(DbSnafu)?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO files (root, rel_path, hash, mtime_ms, size) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .context(DbSnafu)?;
            for entry in &manifest.entries {
                // SQLite columns are signed INTEGER and rusqlite 0.40 dropped its
                // `u64` binders, so convert. An mtime/size past `i64::MAX` cannot
                // occur in practice; fail the write rather than wrap it negative.
                let mtime_ms = i64::try_from(entry.mtime_ms)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
                    .context(DbSnafu)?;
                let size = i64::try_from(entry.size)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
                    .context(DbSnafu)?;
                stmt.execute(params![
                    root.as_ref(),
                    entry.rel_path,
                    entry.hash.as_str(),
                    mtime_ms,
                    size,
                ])
                .context(DbSnafu)?;
            }
        }
        tx.commit().context(DbSnafu)?;
        Ok(())
    }

    /// The content signature last successfully synced for `root` to `store`, if
    /// any. A match against the current manifest's signature means the store
    /// already holds this checkout's content, so sync can skip the network.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn synced_signature(&self, root: &Path, store: &str) -> Result<Option<String>> {
        let root = root.to_string_lossy();
        self.conn
            .query_row(
                "SELECT signature FROM synced WHERE root = ?1 AND store = ?2",
                params![root.as_ref(), store],
                |row| row.get::<_, String>(0),
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
            .context(DbSnafu)
    }

    /// Record that `root` was successfully synced to `store` at `signature`.
    /// Keyed by `(root, store)`, so switching stores forces a fresh sync rather
    /// than trusting another store's state.
    ///
    /// # Errors
    /// Returns an error if the upsert fails.
    pub fn mark_synced(&mut self, root: &Path, store: &str, signature: &str) -> Result<()> {
        let root = root.to_string_lossy();
        self.conn
            .execute(
                "INSERT INTO synced (root, store, signature) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(root, store) DO UPDATE SET signature = excluded.signature",
                params![root.as_ref(), store, signature],
            )
            .context(DbSnafu)?;
        Ok(())
    }
}

/// Path to the shared manifest database, `<cache>/semantic-search/index.db`.
///
/// # Errors
/// Returns an error if no user cache directory can be determined.
pub fn db_path() -> Result<PathBuf> {
    let base = dirs::cache_dir().context(NoCacheDirSnafu)?;
    Ok(base.join("semantic-search").join("index.db"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::Db;
    use crate::content::ContentHash;
    use crate::manifest::{FileEntry, Manifest};

    fn entry(rel_path: &str, content: &str) -> FileEntry {
        FileEntry {
            rel_path: rel_path.to_owned(),
            hash: ContentHash::of_bytes(content.as_bytes()),
            mtime_ms: 1,
            size: 1,
        }
    }

    fn manifest(entries: Vec<FileEntry>) -> Manifest {
        Manifest { entries }
    }

    fn open(dir: &tempfile::TempDir) -> Db {
        Db::open_at(&dir.path().join("index.db")).expect("open db")
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut db = open(&dir);
        let root = Path::new("/repo/a");

        db.save(
            root,
            &manifest(vec![entry("a.rs", "one"), entry("b.rs", "two")]),
        )
        .expect("save");
        let loaded = db.load(root).expect("load");

        assert_eq!(loaded.entries.len(), 2);
        let by_path: Vec<&str> = loaded.entries.iter().map(|e| e.rel_path.as_str()).collect();
        assert!(by_path.contains(&"a.rs") && by_path.contains(&"b.rs"));
    }

    #[test]
    fn roots_are_isolated() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut db = open(&dir);

        db.save(
            Path::new("/repo/a"),
            &manifest(vec![entry("a.rs", "x"), entry("b.rs", "y")]),
        )
        .expect("save a");
        db.save(Path::new("/repo/b"), &manifest(vec![entry("a.rs", "z")]))
            .expect("save b");

        assert_eq!(db.load(Path::new("/repo/a")).expect("a").entries.len(), 2);
        assert_eq!(db.load(Path::new("/repo/b")).expect("b").entries.len(), 1);
    }

    #[test]
    fn save_removes_stale_rows_for_that_root() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut db = open(&dir);
        let root = Path::new("/repo/a");

        db.save(
            root,
            &manifest(vec![entry("a.rs", "x"), entry("b.rs", "y")]),
        )
        .expect("first");
        db.save(root, &manifest(vec![entry("a.rs", "x")]))
            .expect("second");

        let loaded = db.load(root).expect("load");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].rel_path, "a.rs");
    }

    #[test]
    fn synced_signature_round_trips_and_is_store_scoped() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut db = open(&dir);
        let root = Path::new("/repo/a");

        // Absent until recorded.
        assert_eq!(db.synced_signature(root, "s1").expect("query"), None);

        db.mark_synced(root, "s1", "sig-1").expect("mark");
        assert_eq!(
            db.synced_signature(root, "s1").expect("query").as_deref(),
            Some("sig-1")
        );
        // A different store does not inherit s1's state, so switching stores
        // forces a fresh sync.
        assert_eq!(db.synced_signature(root, "s2").expect("query"), None);

        // Re-marking the same (root, store) overwrites in place.
        db.mark_synced(root, "s1", "sig-2").expect("re-mark");
        assert_eq!(
            db.synced_signature(root, "s1").expect("query").as_deref(),
            Some("sig-2")
        );
    }

    #[test]
    fn data_persists_across_reopen() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = Path::new("/repo/a");
        {
            let mut db = open(&dir);
            db.save(root, &manifest(vec![entry("a.rs", "x")]))
                .expect("save");
        }
        let db = open(&dir);
        assert_eq!(db.load(root).expect("load").entries.len(), 1);
    }
}
