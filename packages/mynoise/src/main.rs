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

use anyhow::{Context as _, Result, bail};
use clap::Parser;

use crate::audio::Band;

/// Play a myNoise.net generator by mixing its band loops locally.
#[derive(Parser)]
#[command(version, about)]
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

    let Some(name) = cli.name.as_deref() else {
        bail!("provide a generator name (or use --list); see --help");
    };

    let cache_dir = cli
        .cache_dir
        .or_else(|| dirs::cache_dir().map(|d| d.join("mynoise")))
        .context("could not determine a cache directory; pass --cache-dir")?;

    let (code, files) = rt.block_on(async {
        let code = resolve::resolve_code(&client, name).await?;
        let files = resolve::download_bands(&client, &code, &cache_dir).await?;
        Ok::<_, anyhow::Error>((code, files))
    })?;

    let master = f32::from(cli.volume) / 100.0;
    let bands: Vec<Band> = files
        .into_iter()
        .enumerate()
        .map(|(i, path)| {
            let gain = cli.gains.get(i).copied().unwrap_or(cli.default_gain);
            Band {
                path,
                amplitude: f32::from(gain) / 100.0 * master,
            }
        })
        .collect();

    eprintln!(
        "Playing {code} ({} bands) at volume {}. Ctrl-C to stop.",
        bands.len(),
        cli.volume
    );
    audio::play(&bands)
}
