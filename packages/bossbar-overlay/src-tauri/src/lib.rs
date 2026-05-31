//! Minecraft-style boss bar overlay backend.
//!
//! The overlay's only data source is a single SQLite file. Anyone can write to
//! it (`sqlite3 bossbars.db "INSERT ..."`) and the change shows up on screen.
//! We detect cross-process writes with `PRAGMA data_version`, which bumps
//! whenever *another* connection commits, so a 200ms poll is cheap and never
//! misses a WAL write the way file-mtime watching does.

use std::{
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use rusqlite::Connection;
use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, PhysicalPosition, PhysicalSize,
};
use tauri_plugin_opener::OpenerExt;

/// One boss bar, shaped to mirror Minecraft's own boss bar API.
#[derive(Serialize, Clone, Debug)]
struct BossBar {
    id: i64,
    title: String,
    /// 0.0..=1.0 fill fraction.
    progress: f64,
    /// pink | blue | red | green | yellow | purple | white
    color: String,
    /// progress | notched_6 | notched_10 | notched_12 | notched_20
    overlay: String,
    position: i64,
}

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

fn resolve_db_path() -> PathBuf {
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

fn open_db(path: &Path) -> rusqlite::Result<Connection> {
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

fn read_bars(conn: &Connection) -> rusqlite::Result<Vec<BossBar>> {
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
            progress: r.get::<_, f64>(2)?.clamp(0.0, 1.0),
            color: r.get(3)?,
            overlay: r.get(4)?,
            position: r.get(5)?,
        })
    })?;
    rows.collect()
}

fn data_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("PRAGMA data_version", [], |r| r.get(0))
}

/// Background loop: re-read bars whenever the DB changes and push them to the
/// webview as a `bossbars` event.
fn spawn_watcher(app: tauri::AppHandle, db: PathBuf) {
    thread::spawn(move || {
        let mut conn = match open_db(&db) {
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
                        match read_bars(c) {
                            Ok(bars) => {
                                let _ = app.emit("bossbars", bars);
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
                    conn = open_db(&db).ok();
                }
            }
            thread::sleep(Duration::from_millis(200));
        }
    });
}

/// Initial paint without waiting for the first poll tick.
#[tauri::command]
fn get_bars(db: tauri::State<'_, PathBuf>) -> Result<Vec<BossBar>, String> {
    let conn = open_db(&db).map_err(|e| e.to_string())?;
    read_bars(&conn).map_err(|e| e.to_string())
}

/// Stretch the window across the top of the primary monitor and make it a true
/// pass-through overlay (transparent, always-on-top, ignores the cursor).
fn configure_overlay(app: &tauri::App) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    if let Ok(Some(monitor)) = win.primary_monitor() {
        let size = monitor.size();
        let height = ((size.height as f64) * 0.45) as u32;
        let _ = win.set_size(PhysicalSize::new(size.width, height));
        let _ = win.set_position(PhysicalPosition::new(0, 0));
    }
    let _ = win.set_ignore_cursor_events(true);
    let _ = win.set_visible_on_all_workspaces(true);
}

fn build_tray(app: &tauri::App, db: PathBuf) -> tauri::Result<()> {
    let header = MenuItem::with_id(app, "header", "Boss Bar Overlay", false, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let open = MenuItem::with_id(app, "open_folder", "Open database folder", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&header, &sep, &open, &quit])?;

    let folder = db
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| db.clone());

    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip(format!("Boss Bar Overlay\n{}", db.display()))
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "quit" => app.exit(0),
            "open_folder" => {
                let _ = app.opener().open_path(folder.to_string_lossy(), None::<&str>);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_seeds_and_reads_back() {
        let dir = std::env::temp_dir().join(format!("bb-test-seed-{}", std::process::id()));
        let path = dir.join("bossbars.db");
        let _ = std::fs::remove_dir_all(&dir);

        let conn = open_db(&path).unwrap();
        let bars = read_bars(&conn).unwrap();
        assert_eq!(bars.len(), 3, "fresh DB should seed 3 example bars");
        assert_eq!(bars[0].title, "Ender Dragon");
        assert!((bars[0].progress - 0.82).abs() < 1e-9);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn out_of_range_progress_is_clamped() {
        let dir = std::env::temp_dir().join(format!("bb-test-clamp-{}", std::process::id()));
        let path = dir.join("bossbars.db");
        let _ = std::fs::remove_dir_all(&dir);

        let conn = open_db(&path).unwrap();
        conn.execute("DELETE FROM bossbars", []).unwrap();
        conn.execute(
            "INSERT INTO bossbars (title, progress) VALUES ('hi', 5.0), ('lo', -2.0)",
            [],
        )
        .unwrap();
        let bars = read_bars(&conn).unwrap();
        assert!(bars.iter().all(|b| (0.0..=1.0).contains(&b.progress)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_version_detects_another_connections_write() {
        // This is the invariant the whole watcher relies on: a commit from a
        // *different* connection must bump our reader's PRAGMA data_version.
        let dir = std::env::temp_dir().join(format!("bb-test-ver-{}", std::process::id()));
        let path = dir.join("bossbars.db");
        let _ = std::fs::remove_dir_all(&dir);

        let reader = open_db(&path).unwrap();
        let before = data_version(&reader).unwrap();

        let writer = Connection::open(&path).unwrap();
        writer
            .execute("INSERT INTO bossbars (title) VALUES ('new')", [])
            .unwrap();

        let after = data_version(&reader).unwrap();
        assert_ne!(before, after, "writer's commit must change data_version");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db = resolve_db_path();
    println!("bossbar-overlay: database at {}", db.display());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(db.clone())
        .invoke_handler(tauri::generate_handler![get_bars])
        .setup(move |app| {
            // Live in the menu bar only: Accessory keeps the tray icon and the
            // overlay window but drops the Dock icon and app-switcher entry, so
            // a background HUD does not take a Dock slot.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            configure_overlay(app);
            build_tray(app, db.clone())?;
            spawn_watcher(app.handle().clone(), db.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
