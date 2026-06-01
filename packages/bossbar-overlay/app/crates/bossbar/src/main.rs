//! Minecraft-style boss bar desktop overlay.
//!
//! A native winit + wgpu overlay (no webview): a transparent, always-on-top,
//! click-through window that draws Minecraft boss bars across the top of the
//! screen, driven entirely by a single SQLite file. Write rows into it from
//! anything and they appear within ~200ms.
//!
//! Usage:
//!   bossbar-overlay                 run the overlay
//!   bossbar-overlay --snapshot OUT  render the current bars to a PNG and exit
//!                   [--scale N] [--size WxH]

mod assets;
mod bars;
mod db;
#[cfg(target_os = "linux")]
mod layer_shell;
mod overlay;
mod scene;
mod snapshot;

use std::path::PathBuf;

/// Default logical pixel scale of the 182x5 sprites; overridable with
/// `BOSSBAR_SCALE` or `--scale`.
const DEFAULT_SCALE: u32 = 2;

struct Args {
    snapshot: Option<PathBuf>,
    scale: u32,
    width: u32,
    height: u32,
}

fn parse_args() -> Result<Args, String> {
    let scale = std::env::var("BOSSBAR_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SCALE);
    let mut args = Args {
        snapshot: None,
        scale,
        width: 800,
        height: 280,
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
            "--size" => {
                let v = it.next().ok_or("--size needs WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size must be WxH")?;
                args.width = w.parse().map_err(|_| "bad --size width")?;
                args.height = h.parse().map_err(|_| "bad --size height")?;
            }
            "-h" | "--help" => {
                println!(
                    "bossbar-overlay [--snapshot OUT] [--scale N] [--size WxH]\n\
                     SQLite-driven Minecraft boss bar overlay. DB path: BOSSBAR_DB \
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
            eprintln!("bossbar-overlay: {e}");
            std::process::exit(2);
        }
    };

    let db = db::resolve_path();

    if let Some(out) = args.snapshot {
        let bars = db::read_once(&db).unwrap_or_default();
        match snapshot::run(args.scale.max(1), args.width, args.height, &bars, &out) {
            Ok(()) => println!("bossbar-overlay: wrote {}", out.display()),
            Err(e) => {
                eprintln!("bossbar-overlay: snapshot failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    println!("bossbar-overlay: database at {}", db.display());
    if let Err(e) = overlay::run(db, args.scale) {
        eprintln!("bossbar-overlay: {e}");
        std::process::exit(1);
    }
}
