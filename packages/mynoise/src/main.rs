//! `mynoise`: play [myNoise.net](https://mynoise.net) generators from the CLI.
//!
//! myNoise has no server-side audio generation. Each generator is a handful of
//! stereo OGG loops served as static files at
//! `https://mynoise.net/Data/<CODE>/<n>a.ogg`, one per frequency band; the
//! website's sliders are pure per-band volume mixed in the browser. This tool
//! resolves a name to its `<CODE>`, downloads the bands (cached locally), then
//! loops and mixes them with the same per-band gains.
//!
//! The audio is © Stéphane Pigeon (mynoise.net), patron-funded. Streaming it
//! for personal listening is fine; redistribution is not.

mod audio;
mod resolve;

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::{ArgGroup, Parser};

use crate::audio::Band;

/// Play a myNoise.net generator by mixing its band loops locally.
#[derive(Parser)]
#[command(version, about)]
// Bare `mynoise` prints help instead of erroring; otherwise clap requires
// exactly one of a generator name or `--list`, so it owns the "no target"
// usage error (clean exit 2, no anyhow backtrace).
#[command(arg_required_else_help = true)]
#[command(group(ArgGroup::new("target").required(true).args(["name", "list"])))]
struct Cli {
    /// Generator to play: a bare data code (`RAIN`, `OSMOSIS`) or a generator
    /// page slug (`rainNoiseGenerator`). Omit together with `--list`.
    name: Option<String>,

    /// Per-band gains in band order, each 0..=100. Bands without an explicit
    /// value use `--default-gain`; extra values past the band count are ignored.
    gains: Vec<u8>,

    /// List the generator slugs scraped from the myNoise index and exit.
    #[arg(long)]
    list: bool,

    /// Master volume applied on top of every per-band gain, 0..=100. Lower it if
    /// a full mix clips.
    #[arg(long, default_value_t = 50)]
    volume: u8,

    /// Gain (0..=100) for any band without an explicit value in `gains`.
    #[arg(long, default_value_t = 70)]
    default_gain: u8,

    /// Cache directory for downloaded band files. Defaults to the OS cache dir.
    #[arg(long)]
    cache_dir: Option<PathBuf>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let client = reqwest::Client::builder()
        // A real UA avoids the index/page scrape being served a bot stub.
        .user_agent("mynoise-cli (https://github.com/indexable-inc/index)")
        .build()
        .context("build HTTP client")?;

    // One current-thread runtime drives the network phase; playback is blocking.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build async runtime")?;

    if cli.list {
        let slugs = rt.block_on(resolve::list_slugs(&client))?;
        for slug in slugs {
            println!("{slug}");
        }
        return Ok(());
    }

    // The required `target` group guarantees a name here when `--list` is unset.
    let name = cli
        .name
        .as_deref()
        .expect("clap requires a name unless --list");

    let cache_dir = cli
        .cache_dir
        .or_else(|| dirs::cache_dir().map(|d| d.join("mynoise")))
        .context("could not determine a cache directory; pass --cache-dir")?;

    let (code, files) = rt.block_on(async {
        let code = resolve::resolve_code(&client, name).await?;
        let files = resolve::download_bands(&client, &code, &cache_dir).await?;
        Ok::<_, anyhow::Error>((code, files))
    })?;

    // Resolve the per-band gains (explicit value, else the default) in band order.
    let gains: Vec<f32> = files
        .iter()
        .enumerate()
        .map(|(i, _)| f32::from(cli.gains.get(i).copied().unwrap_or(cli.default_gain)) / 100.0)
        .collect();

    // The bands are decorrelated noise, so their power (not their amplitude) adds;
    // dividing the master by sqrt(active band count) keeps a full mix from
    // overshooting full scale and clipping in rodio's mixer, while holding
    // roughly constant perceived loudness as the band count changes.
    let active = gains.iter().filter(|g| **g > 0.0).count().max(1);
    #[allow(clippy::cast_precision_loss)]
    let master = f32::from(cli.volume) / 100.0 / (active as f32).sqrt();

    let bands: Vec<Band> = files
        .into_iter()
        .zip(gains)
        .map(|(path, gain)| Band {
            path,
            amplitude: gain * master,
        })
        .collect();

    eprintln!(
        "Playing {code} ({} bands) at volume {}. Ctrl-C to stop.",
        bands.len(),
        cli.volume
    );
    audio::play(&bands)
}
