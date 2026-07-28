//! Minecraft-style book desktop overlay.
//!
//! A native winit + wgpu overlay (no webview): a transparent, always-on-top,
//! click-through window that draws an open Minecraft book as a two-page spread,
//! driven entirely by a single SQLite file. Write pages into it from anything and
//! they appear within ~200ms. Shares the float window and the wgpu pixel/text
//! engine with the boss bar overlay via the `overlay-core` crate.
//!
//! Usage:
//!   book-overlay                 run the overlay
//!   book-overlay --snapshot OUT  render the current spread to a PNG and exit
//!                [--scale N] [--page N] [--hover]

mod assets;
mod book;
mod db;
mod overlay;
mod scene;
mod sound;

use std::path::PathBuf;

use book::Book;

/// Default logical pixel scale of the book sprite; overridable with `BOOK_SCALE`
/// or `--scale`. The book art is larger than the bars, so it defaults smaller.
const DEFAULT_SCALE: u32 = 3;

struct Args {
    snapshot: Option<PathBuf>,
    scale: u32,
    /// Left page of the spread to snapshot (0-based; clamped to the book).
    page: usize,
    /// Render the snapshot in the hovered state (book grown, forward arrow
    /// highlighted), so the PNG verifies the hover styling the live overlay only
    /// shows under the pointer.
    hover: bool,
}

fn parse_args() -> Result<Args, String> {
    let scale = std::env::var("BOOK_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SCALE);
    let mut args = Args {
        snapshot: None,
        scale,
        page: 0,
        hover: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--snapshot" => {
                let p = it.next().ok_or("--snapshot needs a path")?;
                args.snapshot = Some(PathBuf::from(p));
            }
            "--scale" => {
                args.scale = it
                    .next()
                    .ok_or("--scale needs a number")?
                    .parse()
                    .map_err(|_| "--scale must be an integer")?;
            }
            "--page" => {
                args.page = it
                    .next()
                    .ok_or("--page needs a number")?
                    .parse()
                    .map_err(|_| "--page must be an integer")?;
            }
            "--hover" => args.hover = true,
            "-h" | "--help" => {
                println!(
                    "book-overlay [--snapshot OUT] [--scale N] [--page N] [--hover]\n\
                     SQLite-driven Minecraft book overlay. DB path: BOOK_DB or the \
                     per-OS app-data path."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("book-overlay: {e}");
            std::process::exit(2);
        }
    };

    let db = db::resolve_path();

    if let Some(out) = args.snapshot {
        let book = db::read_once(&db).unwrap_or(Book {
            pages: vec![String::new()],
            pos: None,
        });
        let scale = args.scale.max(1);
        // Snapshot starts a spread on an even page, the same as the live overlay.
        let spread = (args.page - (args.page % 2)).min(book.last_spread());
        let (w, h) = scene::spread_window_px(scale);
        // At rest by default; `--hover` showcases the hover styling (whole-book
        // grow plus the forward arrow highlighted and popped) so the PNG can
        // verify what the live overlay only reveals under the pointer.
        let hover = if args.hover {
            scene::Hover {
                book: 1.0,
                back: 0.0,
                fwd: 1.0,
            }
        } else {
            scene::Hover::default()
        };
        let result = overlay_core::snapshot::render_to_png(
            w,
            h,
            |gpu| {
                let tex = scene::register(gpu);
                scene::build(
                    gpu,
                    &tex,
                    &book,
                    spread,
                    scale,
                    w,
                    h,
                    spread > 0,
                    spread + 2 <= book.last_spread(),
                    &hover,
                )
            },
            &out,
        );
        match result {
            Ok(()) => println!("book-overlay: wrote {}", out.display()),
            Err(e) => {
                eprintln!("book-overlay: snapshot failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    println!("book-overlay: database at {}", db.display());
    if let Err(e) = overlay::run(db, args.scale) {
        eprintln!("book-overlay: {e}");
        std::process::exit(1);
    }
}
