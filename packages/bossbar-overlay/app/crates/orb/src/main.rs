//! Minecraft-style floating experience-orb desktop overlay.
//!
//! A native winit + wgpu overlay (no webview): a transparent, always-on-top,
//! click-through window holding a single Minecraft experience orb that gently bobs
//! and shimmers, driven entirely by a single SQLite file. Set its XP `amount`
//! (which picks the orb's size) and it updates within ~200ms. Shares the float
//! window and the wgpu pixel engine with the boss bar and book overlays via the
//! `overlay-core` crate.
//!
//! Usage:
//!   xp-orb-overlay                 run the overlay
//!   xp-orb-overlay --snapshot OUT  render the current orb to a PNG and exit
//!                  [--scale N] [--amount N] [--hover]

mod assets;
mod db;
mod orb;
mod overlay;
mod scene;

use std::path::PathBuf;

/// Default logical pixel scale of the 16x16 orb sprite; overridable with
/// `ORB_SCALE` or `--scale`.
const DEFAULT_SCALE: u32 = 4;

struct Args {
    snapshot: Option<PathBuf>,
    scale: u32,
    /// XP amount to snapshot (overrides the DB), so a PNG can show any orb size.
    amount: Option<i64>,
    /// Render the snapshot in the hovered (grown) state.
    hover: bool,
}

fn parse_args() -> Result<Args, String> {
    let scale = std::env::var("ORB_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SCALE);
    let mut args = Args {
        snapshot: None,
        scale,
        amount: None,
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
            "--amount" => {
                args.amount = Some(
                    it.next()
                        .ok_or("--amount needs a number")?
                        .parse()
                        .map_err(|_| "--amount must be an integer")?,
                );
            }
            "--hover" => args.hover = true,
            "-h" | "--help" => {
                println!(
                    "xp-orb-overlay [--snapshot OUT] [--scale N] [--amount N] [--hover]\n\
                     SQLite-driven Minecraft experience-orb overlay. DB path: ORB_DB \
                     or the per-OS app-data path."
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
            eprintln!("xp-orb-overlay: {e}");
            std::process::exit(2);
        }
    };

    let db = db::resolve_path();

    if let Some(out) = args.snapshot {
        let mut orb = db::read_once(&db).unwrap_or_default();
        if let Some(a) = args.amount {
            orb.amount = a.max(0);
        }
        let scale = args.scale.max(1);
        let (w, h) = scene::orb_window_px(scale);
        let hover = if args.hover { 1.0 } else { 0.0 };
        // A still frame: mid-shimmer, no bob, so the PNG is deterministic.
        let result = overlay_core::snapshot::render_to_png(
            w,
            h,
            |gpu| {
                let tex = scene::register(gpu);
                scene::build(&tex, &orb, scale, w, h, hover, 0.5, 0.0)
            },
            &out,
        );
        match result {
            Ok(()) => println!("xp-orb-overlay: wrote {}", out.display()),
            Err(e) => {
                eprintln!("xp-orb-overlay: snapshot failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    println!("xp-orb-overlay: database at {}", db.display());
    if let Err(e) = overlay::run(db, args.scale) {
        eprintln!("xp-orb-overlay: {e}");
        std::process::exit(1);
    }
}
