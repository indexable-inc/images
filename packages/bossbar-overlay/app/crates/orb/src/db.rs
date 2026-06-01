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
);
CREATE TABLE IF NOT EXISTS events (
  id      INTEGER PRIMARY KEY AUTOINCREMENT,
  text    TEXT    NOT NULL DEFAULT '',
  amount  INTEGER NOT NULL DEFAULT 7,
  kind    TEXT    NOT NULL DEFAULT 'orb',
  created INTEGER NOT NULL DEFAULT (unixepoch())
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
    ensure_kind_column(&conn)?;
    if fresh {
        conn.execute_batch(SEED)?;
    }
    Ok(conn)
}

/// Add the `events.kind` column to a DB created before kinds existed. `CREATE
/// TABLE IF NOT EXISTS` never alters an existing table, so an older local DB
/// keeps the column-less schema until we add it here; new DBs already have it
/// from `SCHEMA` and this is a no-op. Checked via `pragma_table_info` rather than
/// catching a "duplicate column" error string.
fn ensure_kind_column(conn: &Connection) -> rusqlite::Result<()> {
    let has_kind = conn
        .prepare("SELECT 1 FROM pragma_table_info('events') WHERE name = 'kind'")?
        .exists([])?;
    if !has_kind {
        conn.execute_batch("ALTER TABLE events ADD COLUMN kind TEXT NOT NULL DEFAULT 'orb';")?;
    }
    Ok(())
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

/// One queued announcement: a labelled sprite to float up once. `amount` picks
/// the orb size via [`crate::scene::icon_for`] (ignored by the villager kind);
/// `kind` is the wire form (`"orb"` / `"villager"`) parsed by
/// [`crate::scene::Kind::parse`], defaulting to `orb` for any unknown value.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub id: i64,
    pub text: String,
    pub amount: i64,
    pub kind: String,
}

/// Open a connection for the feed reader (schema ensured). Public so the feed
/// overlay can poll the `events` table on its own connection.
pub fn connect(path: &Path) -> rusqlite::Result<Connection> {
    open(path)
}

/// Queue one announcement. Called by `xp-orb-overlay push` (e.g. from pr-watch on
/// a merged PR, or a watcher on a failure); the running feed overlay picks it up
/// within ~200ms. `kind` is the wire form (`"orb"` / `"villager"`).
pub fn push_event(path: &Path, text: &str, amount: i64, kind: &str) -> rusqlite::Result<()> {
    let conn = open(path)?;
    conn.execute(
        "INSERT INTO events (text, amount, kind) VALUES (?1, ?2, ?3)",
        rusqlite::params![text, amount.max(0), kind],
    )?;
    Ok(())
}

/// Highest event id present, so the feed can seed its cursor on startup and stay
/// quiet about the existing backlog (only newly inserted events animate).
pub fn max_event_id(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |r| r.get(0))
}

/// Events queued after `after` (exclusive), oldest first.
pub fn read_events_after(conn: &Connection, after: i64) -> rusqlite::Result<Vec<Event>> {
    let mut stmt =
        conn.prepare("SELECT id, text, amount, kind FROM events WHERE id > ?1 ORDER BY id ASC")?;
    let rows = stmt.query_map([after], |r| {
        Ok(Event {
            id: r.get(0)?,
            text: r.get(1)?,
            amount: r.get::<_, i64>(2)?.max(0),
            kind: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// Drop consumed events older than `older_than_secs` so the table stays small.
/// Keeps a short tail so a just-restarted overlay does not miss a very recent
/// push, while never replaying the whole history.
pub fn prune_events(conn: &Connection, older_than_secs: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM events WHERE created < unixepoch() - ?1",
        [older_than_secs.max(0)],
    )?;
    Ok(())
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

    #[test]
    fn events_queue_and_read_after_cursor() {
        let path = temp_db("events");
        let _ = open(&path).unwrap();
        push_event(&path, "ix \u{b7} first", 7, "orb").unwrap();
        push_event(&path, "index \u{b7} second", -3, "villager").unwrap();
        let conn = open(&path).unwrap();
        let all = read_events_after(&conn, 0).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].text, "ix \u{b7} first");
        assert_eq!(all[0].kind, "orb");
        assert_eq!(all[1].amount, 0, "negative amount clamps to zero");
        assert_eq!(all[1].kind, "villager");
        // Cursor past the first returns only the newer event.
        let rest = read_events_after(&conn, all[0].id).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].text, "index \u{b7} second");
        assert_eq!(max_event_id(&conn).unwrap(), all[1].id);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn legacy_events_table_migrates_and_defaults_to_orb() {
        // A DB created before the `kind` column existed: build the old events
        // schema by hand, insert a row, then reopen and confirm the migration
        // added `kind` defaulting to "orb".
        let path = temp_db("migrate");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE events (
                   id      INTEGER PRIMARY KEY AUTOINCREMENT,
                   text    TEXT    NOT NULL DEFAULT '',
                   amount  INTEGER NOT NULL DEFAULT 7,
                   created INTEGER NOT NULL DEFAULT (unixepoch())
                 );
                 INSERT INTO events (text, amount) VALUES ('old row', 5);",
            )
            .unwrap();
        }
        // open() runs the migration; the pre-existing row gets the default kind.
        let conn = open(&path).unwrap();
        let all = read_events_after(&conn, 0).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].text, "old row");
        assert_eq!(all[0].kind, "orb");
        // And a new row still defaults correctly.
        push_event(&path, "new row", 9, "villager").unwrap();
        let rest = read_events_after(&conn, all[0].id).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].kind, "villager");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
