//! SQLite is the overlay's only data source. Anyone can write to it
//! (`sqlite3 bossbars.db "INSERT ..."`, or the `bossbar` CLI) and the change
//! shows up on screen. Cross-process writes are detected with
//! `PRAGMA data_version`, which bumps whenever *another* connection commits, so
//! a 200ms poll is cheap and never misses a WAL write the way file-mtime
//! watching does.

use std::{
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use rusqlite::Connection;

use crate::bars::{BossBar, Color, Overlay};

/// How often the watcher checks `PRAGMA data_version`. The issue's contract is
/// that an external write lands on screen within ~200ms.
pub const POLL: Duration = Duration::from_millis(200);

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS bossbars (
  id        INTEGER PRIMARY KEY,
  title     TEXT    NOT NULL DEFAULT '',
  progress  REAL    NOT NULL DEFAULT 1.0,
  color     TEXT    NOT NULL DEFAULT 'purple',
  overlay   TEXT    NOT NULL DEFAULT 'progress',
  visible   INTEGER NOT NULL DEFAULT 1,
  position  INTEGER NOT NULL DEFAULT 0
);";

/// Rows inserted only when the DB file is created for the first time, so a
/// brand-new install shows something and documents the contract by example.
const SEED: &str = "\
INSERT INTO bossbars (title, progress, color, overlay, position) VALUES
  ('Ender Dragon',  0.82, 'pink',   'notched_20', 0),
  ('Wither',        0.55, 'blue',   'notched_6',  1),
  ('Build: compiling', 0.40, 'green', 'progress',  2);";

/// Resolve the database path: `BOSSBAR_DB` wins, otherwise the per-OS app-data
/// path the `bossbar` CLI also computes.
pub fn resolve_path() -> PathBuf {
    if let Ok(p) = std::env::var("BOSSBAR_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("bossbar-overlay").join("bossbars.db")
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    let fresh = !path.exists();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path)?;
    // WAL lets external `sqlite3` writers commit without blocking our reader.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(SCHEMA)?;
    if fresh {
        conn.execute_batch(SEED)?;
    }
    Ok(conn)
}

fn read(conn: &Connection) -> rusqlite::Result<Vec<BossBar>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, progress, color, overlay, position
         FROM bossbars
         WHERE visible != 0
         ORDER BY position ASC, id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(BossBar {
            id: r.get(0)?,
            title: r.get(1)?,
            progress: r.get::<_, f64>(2)?.clamp(0.0, 1.0) as f32,
            color: Color::parse(&r.get::<_, String>(3)?),
            overlay: Overlay::parse(&r.get::<_, String>(4)?),
            position: r.get(5)?,
        })
    })?;
    rows.collect()
}

fn data_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA data_version", [], |r| r.get(0))
}

/// Read the current bars once, for the first paint before the watcher ticks.
pub fn read_once(path: &Path) -> rusqlite::Result<Vec<BossBar>> {
    let conn = open(path)?;
    read(&conn)
}

/// Background loop: re-read bars whenever the DB changes and hand them to
/// `sink`. The loop exits as soon as `sink` returns `false`, which is how the
/// UI thread signals the window has closed.
pub fn spawn_watcher<F>(db: PathBuf, mut sink: F)
where
    F: FnMut(Vec<BossBar>) -> bool + Send + 'static,
{
    thread::spawn(move || {
        let mut conn = match open(&db) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("bossbar-overlay: failed to open {}: {e}", db.display());
                None
            }
        };
        let mut last_version: Option<i64> = None;

        loop {
            match conn.as_ref() {
                Some(c) => match data_version(c) {
                    Ok(v) if Some(v) != last_version => {
                        last_version = Some(v);
                        match read(c) {
                            Ok(bars) => {
                                if !sink(bars) {
                                    return;
                                }
                            }
                            Err(e) => eprintln!("bossbar-overlay: read failed: {e}"),
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("bossbar-overlay: poll failed, reopening: {e}");
                        conn = None;
                        last_version = None;
                    }
                },
                None => {
                    // DB went away or never opened: retry the open.
                    conn = open(&db).ok();
                }
            }
            thread::sleep(POLL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bb-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("bossbars.db")
    }

    #[test]
    fn fresh_db_seeds_and_reads_back() {
        let path = temp_db("seed");
        let conn = open(&path).unwrap();
        let bars = read(&conn).unwrap();
        assert_eq!(bars.len(), 3, "fresh DB should seed 3 example bars");
        assert_eq!(bars[0].title, "Ender Dragon");
        assert_eq!(bars[0].color, Color::Pink);
        assert_eq!(bars[0].overlay, Overlay::Notched20);
        assert!((bars[0].progress - 0.82).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn out_of_range_progress_is_clamped() {
        let path = temp_db("clamp");
        let conn = open(&path).unwrap();
        conn.execute("DELETE FROM bossbars", []).unwrap();
        conn.execute(
            "INSERT INTO bossbars (title, progress) VALUES ('hi', 5.0), ('lo', -2.0)",
            [],
        )
        .unwrap();
        let bars = read(&conn).unwrap();
        assert!(bars.iter().all(|b| (0.0..=1.0).contains(&b.progress)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn data_version_detects_another_connections_write() {
        // The invariant the whole watcher relies on: a commit from a
        // *different* connection must bump our reader's PRAGMA data_version.
        let path = temp_db("ver");
        let reader = open(&path).unwrap();
        let before = data_version(&reader).unwrap();

        let writer = Connection::open(&path).unwrap();
        writer
            .execute("INSERT INTO bossbars (title) VALUES ('new')", [])
            .unwrap();

        let after = data_version(&reader).unwrap();
        assert_ne!(before, after, "writer's commit must change data_version");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
