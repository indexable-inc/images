//! SQLite is the book overlay's only data source, mirroring the boss bar
//! overlay's contract: anyone can write rows (`sqlite3 book.db "INSERT ..."`) and
//! the change shows up on screen within ~200ms. Cross-process writes are detected
//! with `PRAGMA data_version`, which bumps whenever another connection commits.

use std::{
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension};

use crate::book::Book;

/// Watcher poll interval; an external write lands on screen within ~200ms.
pub const POLL: Duration = Duration::from_millis(200);

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS book (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  x  REAL,
  y  REAL
);
CREATE TABLE IF NOT EXISTS pages (
  id   INTEGER PRIMARY KEY,
  idx  INTEGER NOT NULL DEFAULT 0,
  body TEXT    NOT NULL DEFAULT ''
);";

/// Inserted only when the DB is first created, so a fresh install shows a book and
/// documents the contract by example.
const SEED: &str = "\
INSERT INTO book (id) VALUES (1);
INSERT INTO pages (idx, body) VALUES
  (0, 'Welcome to the book overlay.\n\nIt floats on your desktop like the boss bars, drawn with wgpu from the real Minecraft book texture.'),
  (1, 'Pages live in SQLite. Write rows and they appear, the same contract the boss bar overlay uses.'),
  (2, 'Turn pages with the arrows at the bottom. Drag the book to move it; it remembers where you left it.'),
  (3, 'The book, the arrows, and the font are all extracted from the official Minecraft jar by a Nix derivation.');";

/// Resolve the database path: `BOOK_DB` wins, else the per-OS app-data path.
pub fn resolve_path() -> PathBuf {
    if let Ok(p) = std::env::var("BOOK_DB") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    let base = dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("book-overlay").join("book.db")
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

/// Persist a dragged book's pinned location (logical screen points). Uses its own
/// connection so the overlay's read/watch connection is untouched; the commit
/// bumps `data_version`, so the watcher re-reads and the move sticks.
pub fn set_position(path: &Path, pos: glam::DVec2) -> rusqlite::Result<()> {
    let conn = Connection::open(path)?;
    conn.execute(
        "UPDATE book SET x = ?1, y = ?2 WHERE id = 1",
        rusqlite::params![pos.x, pos.y],
    )?;
    Ok(())
}

fn read(conn: &Connection) -> rusqlite::Result<Book> {
    let (x, y): (Option<f64>, Option<f64>) = conn
        .query_row("SELECT x, y FROM book WHERE id = 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .unwrap_or((None, None));

    let mut stmt = conn.prepare("SELECT body FROM pages ORDER BY idx ASC, id ASC")?;
    let pages: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    let pages = if pages.is_empty() {
        vec![String::new()]
    } else {
        pages
    };

    Ok(Book {
        pages,
        // Both coordinates must be present to pin the book; a half-written row
        // falls back to centering rather than placing it at an edge.
        pos: x.zip(y).map(|(x, y)| glam::DVec2::new(x, y)),
    })
}

fn data_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA data_version", [], |r| r.get(0))
}

/// Read the current book once, for the first paint before the watcher ticks.
pub fn read_once(path: &Path) -> rusqlite::Result<Book> {
    let conn = open(path)?;
    read(&conn)
}

/// Background loop: re-read the book whenever the DB changes and hand it to
/// `sink`. Exits as soon as `sink` returns `false` (the window closed).
pub fn spawn_watcher<F>(db: PathBuf, mut sink: F)
where
    F: FnMut(Book) -> bool + Send + 'static,
{
    thread::spawn(move || {
        let mut conn = match open(&db) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("book-overlay: failed to open {}: {e}", db.display());
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
                            Ok(book) => {
                                if !sink(book) {
                                    return;
                                }
                            }
                            Err(e) => eprintln!("book-overlay: read failed: {e}"),
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("book-overlay: poll failed, reopening: {e}");
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
        let dir = std::env::temp_dir().join(format!("book-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("book.db")
    }

    #[test]
    fn fresh_db_seeds_a_four_page_book() {
        let path = temp_db("seed");
        let conn = open(&path).unwrap();
        let book = read(&conn).unwrap();
        assert_eq!(book.pages.len(), 4);
        assert!(book.pages[0].contains("book overlay"));
        assert_eq!(book.last_spread(), 2, "4 pages -> last spread starts at page index 2");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn empty_book_still_has_one_page() {
        let path = temp_db("empty");
        let conn = open(&path).unwrap();
        conn.execute("DELETE FROM pages", []).unwrap();
        let book = read(&conn).unwrap();
        assert_eq!(book.pages.len(), 1);
        assert_eq!(book.last_spread(), 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn position_round_trips() {
        let path = temp_db("pos");
        let _ = open(&path).unwrap();
        set_position(&path, glam::DVec2::new(12.0, 34.0)).unwrap();
        let conn = open(&path).unwrap();
        let book = read(&conn).unwrap();
        assert_eq!(book.pos, Some(glam::DVec2::new(12.0, 34.0)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
