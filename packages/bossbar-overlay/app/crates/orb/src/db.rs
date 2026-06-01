//! SQLite is the experience-orb overlay's only data source, mirroring the boss bar
//! and book overlays: anyone can write the single `orb` row
//! (`sqlite3 orb.db "UPDATE orb SET amount = 137"`) and the change shows up within
//! ~200ms. Cross-process writes are detected with `PRAGMA data_version`, which
//! bumps whenever another connection commits.

use std::{
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension};

use crate::orb::Orb;

/// Watcher poll interval; an external write lands on screen within ~200ms.
pub const POLL: Duration = Duration::from_millis(200);

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS orb (
  id     INTEGER PRIMARY KEY CHECK (id = 1),
  amount INTEGER NOT NULL DEFAULT 0,
  url    TEXT    NOT NULL DEFAULT '',
  x      REAL,
  y      REAL
);";

/// Inserted only when the DB is first created, so a fresh install shows an orb and
/// documents the contract by example.
const SEED: &str = "\
INSERT INTO orb (id, amount, url) VALUES (1, 137, '');";

/// Resolve the database path: `ORB_DB` wins, else the per-OS app-data path.
pub fn resolve_path() -> PathBuf {
    if let Ok(p) = std::env::var("ORB_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("xp-orb-overlay").join("orb.db")
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

/// Persist the orb's pinned location (logical screen points). Its own connection
/// keeps the overlay's read/watch connection untouched; the commit bumps
/// `data_version`, so the watcher re-reads and the move sticks.
pub fn set_position(path: &Path, pos: glam::DVec2) -> rusqlite::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute(
        "UPDATE orb SET x = ?1, y = ?2 WHERE id = 1",
        rusqlite::params![pos.x, pos.y],
    )?;
    Ok(())
}

fn read(conn: &Connection) -> rusqlite::Result<Orb> {
    conn.query_row(
        "SELECT amount, url, x, y FROM orb WHERE id = 1",
        [],
        |r| {
            let amount: i64 = r.get(0)?;
            let url: String = r.get(1)?;
            let x: Option<f64> = r.get(2)?;
            let y: Option<f64> = r.get(3)?;
            Ok(Orb {
                amount: amount.max(0),
                url,
                // Both coordinates must be present to pin the orb; a half-written
                // row falls back to centering rather than placing it at an edge.
                pos: x.zip(y).map(|(x, y)| glam::DVec2::new(x, y)),
            })
        },
    )
    .optional()
    .map(Option::unwrap_or_default)
}

fn data_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA data_version", [], |r| r.get(0))
}

/// Read the current orb once, for the first paint before the watcher ticks.
pub fn read_once(path: &Path) -> rusqlite::Result<Orb> {
    let conn = open(path)?;
    read(&conn)
}

/// Background loop: re-read the orb whenever the DB changes and hand it to `sink`.
/// Exits as soon as `sink` returns `false` (the window closed).
pub fn spawn_watcher<F>(db: PathBuf, mut sink: F)
where
    F: FnMut(Orb) -> bool + Send + 'static,
{
    thread::spawn(move || {
        let mut conn = match open(&db) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("xp-orb-overlay: failed to open {}: {e}", db.display());
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
                            Ok(orb) => {
                                if !sink(orb) {
                                    return;
                                }
                            }
                            Err(e) => eprintln!("xp-orb-overlay: read failed: {e}"),
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("xp-orb-overlay: poll failed, reopening: {e}");
                        conn = None;
                        last_version = None;
                    }
                },
                None => conn = open(&db).ok(),
            }
            thread::sleep(POLL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orb-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("orb.db")
    }

    #[test]
    fn fresh_db_seeds_one_orb() {
        let path = temp_db("seed");
        let conn = open(&path).unwrap();
        let orb = read(&conn).unwrap();
        assert_eq!(orb.amount, 137);
        assert_eq!(orb.pos, None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn position_round_trips() {
        let path = temp_db("pos");
        let _ = open(&path).unwrap();
        set_position(&path, glam::DVec2::new(12.0, 34.0)).unwrap();
        let conn = open(&path).unwrap();
        assert_eq!(read(&conn).unwrap().pos, Some(glam::DVec2::new(12.0, 34.0)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn negative_amount_clamps_to_zero() {
        let path = temp_db("neg");
        let conn = open(&path).unwrap();
        conn.execute("UPDATE orb SET amount = -5 WHERE id = 1", []).unwrap();
        assert_eq!(read(&conn).unwrap().amount, 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
