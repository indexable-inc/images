//! Record a terminal demo reel as code.
//!
//! `reel` drives a real CLI session through the [`tui`] PTY driver, samples the
//! VT-rendered grid of styled cells over time, rasterizes each frame to RGBA
//! with a flat palette and a vendored monospace face, and muxes the frames with
//! ffmpeg into an animated AVIF (with a WebP fallback). The output is a dark and
//! a light variant sized for a GitHub README `<picture>` element.
//!
//! The pieces:
//! - [`theme`] owns the flat dark/light palettes and color resolution.
//! - [`font`] owns the four embedded JetBrains Mono faces and the glyph cache.
//! - [`raster`] turns one [`scene::Frame`] into an RGBA buffer.
//! - [`scene`] is the recorded script plus the title and outro cards.
//! - [`record`] drives the shell and collects the terminal frames.
//! - [`encode`] streams rendered frames to ffmpeg.

// Pixel math converts freely between `usize` grid indices, `u32` pixel
// coordinates, and `f32` font metrics. Auditing every cast individually adds
// noise without catching a real defect in a fixed-size raster, so the lossy-cast
// lints are relaxed crate-wide here rather than at hundreds of call sites.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "raster code mixes usize indices, u32 pixels, and f32 metrics by design"
)]
// Module and item docs name products and acronyms (JetBrains Mono, WebP, RGBA,
// PTY, GitHub) as prose, not code.
#![allow(
    clippy::doc_markdown,
    reason = "prose docs use product names and acronyms without backticks"
)]

mod encode;
mod font;
mod raster;
mod record;
mod scene;
mod theme;

use std::path::PathBuf;

use clap::Parser;
use color_eyre::eyre::Result;

use crate::encode::{Codec, Encoding, encode};
use crate::font::FontSet;
use crate::raster::Layout;
use crate::record::record;
use crate::scene::{Frame, outro_card, title_card};
use crate::theme::Theme;

/// Record a terminal demo reel to animated AVIF (with a WebP fallback).
#[derive(Parser)]
#[command(name = "reel", version, about)]
struct Cli {
    /// Directory to write the demo-{dark,light}.{avif,webp} files into.
    #[arg(long, default_value = "docs")]
    out_dir: PathBuf,
    /// Output width in pixels; height follows the recorded aspect ratio.
    #[arg(long, default_value_t = 880)]
    width: u32,
    /// Body font size in pixels before downscaling.
    #[arg(long, default_value_t = 30.0)]
    font_size: f32,
    /// Terminal width in columns.
    #[arg(long, default_value_t = 88)]
    cols: u16,
    /// Terminal height in rows.
    #[arg(long, default_value_t = 24)]
    rows: u16,
    /// Capture frame rate for the AVIF (the WebP fallback is capped at 24).
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..))]
    fps: u32,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.out_dir)?;

    eprintln!("reel: recording demo session…");
    let recorded = record(cli.cols, cli.rows, cli.fps)?;

    // Bookend the recording with held title and outro cards.
    let title_hold = (cli.fps as f32 * 2.2) as u32;
    let outro_hold = (cli.fps as f32 * 3.0) as u32;
    let mut frames: Vec<Frame> =
        Vec::with_capacity(recorded.len() + (title_hold + outro_hold) as usize);
    frames.extend(std::iter::repeat_with(|| Frame::Card(title_card())).take(title_hold as usize));
    frames.extend(recorded);
    frames.extend(std::iter::repeat_with(|| Frame::Card(outro_card())).take(outro_hold as usize));

    let mut font = FontSet::new(cli.font_size, 1.34)?;
    let layout = Layout::new(&font, cli.cols as usize, cli.rows as usize);

    // AVIF carries the full frame rate; the WebP fallback is thinned to keep it
    // under GitHub's 10 MB image cap.
    let webp_fps = cli.fps.min(24);
    for theme in [Theme::Dark, Theme::Light] {
        for (codec, output_fps) in [(Codec::Avif, cli.fps), (Codec::Webp, webp_fps)] {
            let encoding = Encoding {
                codec,
                render_fps: cli.fps,
                output_fps,
                width: cli.width,
            };
            let out = cli
                .out_dir
                .join(format!("demo-{}.{}", theme.name(), codec.extension()));
            eprintln!("reel: encoding {}…", out.display());
            encode(&out, &frames, theme, &mut font, &layout, encoding)?;
            println!("wrote {}", out.display());
        }
    }
    Ok(())
}
