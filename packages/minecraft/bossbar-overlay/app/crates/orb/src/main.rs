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
//!   xp-orb-overlay                 run the pinned single-orb overlay
//!   xp-orb-overlay feed            run the full-screen "rise & pop" karma feed
//!   xp-orb-overlay push TEXT       queue one labelled pop for the feed and exit
//!                  [--amount N] [--kind orb|villager]
//!   xp-orb-overlay --snapshot OUT  render a pop/orb to a PNG and exit
//!                  [--scale N] [--amount N] [--hover] [--label TEXT] [--kind K]
//!
//! The feed carries two pop kinds: `orb` (success, the green experience orb +
//! pickup sound) and `villager` (failure, the angry-villager puff + "no" sound).

mod assets;
mod db;
mod feed;
mod orb;
mod overlay;
mod scene;
mod sound;

use std::path::PathBuf;

/// Default logical pixel scale of the 16x16 orb sprite; overridable with
/// `ORB_SCALE` or `--scale`.
const DEFAULT_SCALE: u32 = 4;
/// Default XP amount for a pushed merge orb when `--amount` is omitted: a
/// mid-size orb (icon 2).
const DEFAULT_PUSH_AMOUNT: i64 = 7;

/// Sprite scale from `ORB_SCALE`, else the default.
fn env_scale() -> u32 {
    std::env::var("ORB_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SCALE)
}

struct Args {
    snapshot: Option<PathBuf>,
    scale: u32,
    /// XP amount to snapshot (overrides the DB), so a PNG can show any orb size.
    amount: Option<i64>,
    /// Render the snapshot in the hovered (grown) state.
    hover: bool,
    /// Render the snapshot as a labelled "pop" (sprite + this text) instead of
    /// the bare pinned orb, so the feed look is verifiable from a file.
    label: Option<String>,
    /// Which pop sprite to render with `--label` (orb success / villager failure).
    kind: scene::Kind,
    /// Run the scroll-drag cursor-follow self-test, writing a report here and
    /// exiting. Validates the fix inside the macOS guest VM, where the guest
    /// cursor is invisible to host screenshots, by reading the real cursor in-guest.
    selftest: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        snapshot: None,
        scale: env_scale(),
        amount: None,
        hover: false,
        label: None,
        kind: scene::Kind::Orb,
        selftest: None,
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
            "--label" => {
                args.label = Some(it.next().ok_or("--label needs text")?);
            }
            "--kind" => {
                let k = it.next().ok_or("--kind needs a value")?;
                args.kind = scene::Kind::parse(&k).ok_or("--kind must be orb or villager")?;
            }
            "--selftest" => {
                let p = it.next().ok_or("--selftest needs an output path")?;
                args.selftest = Some(PathBuf::from(p));
            }
            "-h" | "--help" => {
                println!(
                    "xp-orb-overlay                 pinned single-orb overlay\n\
                     xp-orb-overlay feed            full-screen rise-&-pop karma feed\n\
                     xp-orb-overlay push TEXT [--amount N] [--kind orb|villager]   queue one feed pop\n\
                     xp-orb-overlay --snapshot OUT [--scale N] [--amount N] [--hover] [--label TEXT] [--kind K]\n\
                     SQLite-driven Minecraft overlay (orb = success, villager = failure). DB path: \
                     ORB_DB or the per-OS app-data path."
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(args)
}

/// `xp-orb-overlay push TEXT... [--amount N] [--kind orb|villager]`: queue one
/// labelled pop and exit. Positional words join into the label so the caller need
/// not quote. `--kind` defaults to `orb` (success); `villager` is the failure pop.
fn run_push(rest: &[String], db: &std::path::Path) -> ! {
    let mut amount = DEFAULT_PUSH_AMOUNT;
    let mut kind = scene::Kind::Orb;
    let mut parts: Vec<String> = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--amount" => match it.next().and_then(|s| s.parse().ok()) {
                Some(n) => amount = n,
                None => {
                    eprintln!("xp-orb-overlay: push --amount needs a number");
                    std::process::exit(2);
                }
            },
            "--kind" => match it.next().map(|s| scene::Kind::parse(s)) {
                Some(Some(k)) => kind = k,
                _ => {
                    eprintln!("xp-orb-overlay: push --kind must be orb or villager");
                    std::process::exit(2);
                }
            },
            // End of flags: everything after `--` is label text verbatim, so a
            // title that itself contains `--amount` is not mis-parsed.
            "--" => {
                parts.extend(it.by_ref().cloned());
                break;
            }
            other => parts.push(other.to_string()),
        }
    }
    let text = parts.join(" ");
    match db::push_event(db, &text, amount, kind.as_str()) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("xp-orb-overlay: push failed: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let db = db::resolve_path();
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // Subcommands precede the flag-based forms.
    match argv.first().map(String::as_str) {
        Some("feed") => {
            // The feed accepts an optional `--scale N`.
            let mut scale = env_scale();
            let mut it = argv[1..].iter();
            while let Some(a) = it.next() {
                if a == "--scale"
                    && let Some(n) = it.next().and_then(|s| s.parse().ok())
                {
                    scale = n;
                }
            }
            if let Err(e) = feed::run(db, scale) {
                eprintln!("xp-orb-overlay: {e}");
                std::process::exit(1);
            }
            return;
        }
        Some("push") => run_push(&argv[1..], &db),
        _ => {}
    }

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("xp-orb-overlay: {e}");
            std::process::exit(2);
        }
    };

    if let Some(out) = args.selftest {
        match overlay::run_selftest(args.scale, &out) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("xp-orb-overlay: selftest failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if let Some(out) = args.snapshot {
        let scale = args.scale.max(1);
        // A still, mid-shimmer frame so the PNG is deterministic. With --label,
        // render the feed "pop" (orb + label); otherwise the bare pinned orb.
        let result = if let Some(label) = args.label.clone() {
            let amount = args.amount.unwrap_or(DEFAULT_PUSH_AMOUNT).max(0);
            let (pw, ph) = scene::pop_size(&label, scale);
            // Room for the 1px*scale text shadow on the right and bottom.
            let pad = scale;
            let (w, h) = (pw + 2 * pad, ph + 2 * pad);
            overlay_core::snapshot::render_to_png(
                w,
                h,
                |gpu| {
                    let tex = scene::register(gpu);
                    let mut quads = Vec::new();
                    scene::build_pop(
                        gpu, &tex, args.kind, &label, amount, scale, pad as f32, pad as f32, 1.0,
                        0.5, &mut quads,
                    );
                    quads
                },
                &out,
            )
        } else {
            let mut orb = db::read_once(&db).unwrap_or_default();
            if let Some(a) = args.amount {
                orb.amount = a.max(0);
            }
            let (w, h) = scene::orb_window_px(scale);
            let hover = if args.hover { 1.0 } else { 0.0 };
            overlay_core::snapshot::render_to_png(
                w,
                h,
                |gpu| {
                    let tex = scene::register(gpu);
                    scene::build(&tex, &orb, scale, w, h, hover, 0.5, 0.0)
                },
                &out,
            )
        };
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
