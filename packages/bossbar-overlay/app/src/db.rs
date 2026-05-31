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
  id          INTEGER PRIMARY KEY,
  title       TEXT    NOT NULL DEFAULT '',
  description TEXT    NOT NULL DEFAULT '',
  progress    REAL    NOT NULL DEFAULT 1.0,
  color       TEXT    NOT NULL DEFAULT 'purple',
  overlay     TEXT    NOT NULL DEFAULT 'progress',
  visible     INTEGER NOT NULL DEFAULT 1,
  position    INTEGER NOT NULL DEFAULT 0,
  x           REAL,
  y           REAL,
  since       INTEGER,
  url         TEXT    NOT NULL DEFAULT ''
);";

/// Columns added after the initial schema shipped. `ALTER TABLE ADD COLUMN` is
/// the supported online migration in SQLite, and it errors if the column
/// already exists, so each runs through `add_column_if_missing`. A constant
/// default keeps the migration legal on an existing table.
const ADDED_COLUMNS: &[(&str, &str)] = &[
    ("x", "REAL"),
    ("y", "REAL"),
    ("description", "TEXT NOT NULL DEFAULT ''"),
    ("since", "INTEGER"),
    ("url", "TEXT NOT NULL DEFAULT ''"),
];

/// Rows inserted only when the DB file is created for the first time, so a
/// brand-new install shows something and documents the contract by example. The
/// Ender Dragon bar carries a hover panel and a click URL; the Build bar stamps
/// `since` so its title shows a live elapsed counter from first launch.
const SEED: &str = "\
INSERT INTO bossbars (title, description, progress, color, overlay, position, since, url) VALUES
  ('Ender Dragon', 'The final boss of the End. Destroy all the End Crystals on the obsidian pillars first, or it heals back to full.', 0.82, 'pink', 'notched_20', 0, NULL, 'https://minecraft.wiki/w/Ender_Dragon'),
  ('Wither', '', 0.55, 'blue', 'notched_6', 1, NULL, ''),
  ('Build: compiling', 'Hover any bar to read more.\n\nThe panel wraps long lines and keeps your paragraph breaks, so a bar can carry a sentence or a few.', 0.40, 'green', 'progress', 2, strftime('%s','now'), '');";

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
    for (name, ty) in ADDED_COLUMNS {
        add_column_if_missing(&conn, name, ty)?;
    }
    if fresh {
        conn.execute_batch(SEED)?;
    }
    Ok(conn)
}

/// Add a column to `bossbars` only when a pre-existing database lacks it, so
/// upgrading an old DB does not error on the duplicate-column `ALTER TABLE`.
fn add_column_if_missing(conn: &Connection, name: &str, ty: &str) -> rusqlite::Result<()> {
    let present = conn
        .prepare("SELECT 1 FROM pragma_table_info('bossbars') WHERE name = ?1")?
        .exists([name])?;
    if !present {
        // Column names and types here are compile-time constants, never user
        // input, so the format! cannot carry untrusted SQL.
        conn.execute_batch(&format!("ALTER TABLE bossbars ADD COLUMN {name} {ty};"))?;
    }
    Ok(())
}

/// Persist a dragged bar's pinned location (logical screen points). Uses its own
/// connection so the overlay's read/watch connection is untouched; the commit
/// bumps `data_version`, so the watcher re-reads and the move sticks. This runs
/// on the UI thread during a drag, so it deliberately skips the schema/migration
/// setup `open` does: by drag time the table exists, and WAL is a persistent DB
/// property, so a bare connection plus the `UPDATE` keeps each write cheap.
pub fn set_position(path: &Path, id: i64, pos: glam::DVec2) -> rusqlite::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute(
        "UPDATE bossbars SET x = ?1, y = ?2 WHERE id = ?3",
        rusqlite::params![pos.x, pos.y, id],
    )?;
    Ok(())
}

fn read(conn: &Connection) -> rusqlite::Result<Vec<BossBar>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, progress, color, overlay, position, x, y, description, since, url
         FROM bossbars
         WHERE visible != 0
         ORDER BY position ASC, id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        let x: Option<f64> = r.get(6)?;
        let y: Option<f64> = r.get(7)?;
        // A non-positive stored value reads as "no counter", so a caller can clear
        // the clock with `--since 0` without a NULL.
        let since: Option<i64> = r.get::<_, Option<i64>>(9)?.filter(|s| *s > 0);
        Ok(BossBar {
            id: r.get(0)?,
            title: r.get(1)?,
            description: r.get(8)?,
            since,
            url: r.get(10)?,
            progress: r.get::<_, f64>(2)?.clamp(0.0, 1.0) as f32,
            color: Color::parse(&r.get::<_, String>(3)?),
            overlay: Overlay::parse(&r.get::<_, String>(4)?),
            position: r.get(5)?,
            // Both coordinates must be present to pin a bar; a half-written row
            // falls back to auto-stacking rather than placing it at an edge.
            pos: x.zip(y).map(|(x, y)| glam::DVec2::new(x, y)),
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
        // The seed documents the description contract by example.
        assert!(bars[0].description.contains("End Crystals"));
        assert_eq!(bars[1].description, "", "the Wither seed has no panel");
        // The Build seed stamps `since`, so it shows a live counter from launch.
        assert!(bars[2].since.is_some(), "Build seed should have a live counter");
        // The Ender Dragon seed carries a click URL; the others none.
        assert!(bars[0].url.contains("minecraft.wiki"));
        assert_eq!(bars[1].url, "");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn url_round_trips() {
        let path = temp_db("url");
        let conn = open(&path).unwrap();
        conn.execute("DELETE FROM bossbars", []).unwrap();
        conn.execute(
            "INSERT INTO bossbars (title, url) VALUES ('link', 'https://example.com/x'), ('plain', '')",
            [],
        )
        .unwrap();
        let bars = read(&conn).unwrap();
        let by = |t: &str| bars.iter().find(|b| b.title == t).unwrap();
        assert_eq!(by("link").url, "https://example.com/x");
        assert_eq!(by("plain").url, "");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn since_round_trips_and_nonpositive_reads_as_none() {
        let path = temp_db("since");
        let conn = open(&path).unwrap();
        conn.execute("DELETE FROM bossbars", []).unwrap();
        conn.execute(
            "INSERT INTO bossbars (title, since) VALUES ('live', 1700000000), ('zero', 0), ('null', NULL)",
            [],
        )
        .unwrap();
        let bars = read(&conn).unwrap();
        let by = |t: &str| bars.iter().find(|b| b.title == t).unwrap();
        assert_eq!(by("live").since, Some(1_700_000_000));
        // A 0 or NULL is "no counter", so the overlay shows a plain title.
        assert_eq!(by("zero").since, None);
        assert_eq!(by("null").since, None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn legacy_db_without_description_is_migrated_and_round_trips() {
        // A database created before `description` shipped (no description, x, or
        // y columns) must gain the column on open and read back as empty, then
        // accept a written description.
        let path = temp_db("legacy-desc");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let legacy = Connection::open(&path).unwrap();
            legacy
                .execute_batch(
                    "CREATE TABLE bossbars (
                       id       INTEGER PRIMARY KEY,
                       title    TEXT    NOT NULL DEFAULT '',
                       progress REAL    NOT NULL DEFAULT 1.0,
                       color    TEXT    NOT NULL DEFAULT 'purple',
                       overlay  TEXT    NOT NULL DEFAULT 'progress',
                       visible  INTEGER NOT NULL DEFAULT 1,
                       position INTEGER NOT NULL DEFAULT 0
                     );
                     INSERT INTO bossbars (title) VALUES ('old');",
                )
                .unwrap();
        }

        let conn = open(&path).unwrap();
        let bars = read(&conn).unwrap();
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].title, "old");
        assert_eq!(bars[0].description, "", "migrated column defaults to empty");

        conn.execute(
            "UPDATE bossbars SET description = ?1 WHERE id = ?2",
            rusqlite::params!["line one\n\nline two", bars[0].id],
        )
        .unwrap();
        let bars = read(&conn).unwrap();
        assert_eq!(bars[0].description, "line one\n\nline two");
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
