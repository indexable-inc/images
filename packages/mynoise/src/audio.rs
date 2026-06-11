//! Mix a generator's band loops and play them until interrupted.
//!
//! Each band is one OGG loop. The website mixes them in the browser with a
//! per-band volume; we do the same by giving each band its own rodio `Player`
//! on a shared device sink (the sink's mixer sums all players) and looping it
//! forever.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::thread;

use anyhow::{Context as _, Result, bail};
use rodio::source::Source as _;
use rodio::{Decoder, DeviceSinkBuilder, Player};

/// A band file paired with the linear amplitude (0.0..) to play it at.
pub struct Band {
    pub path: PathBuf,
    pub amplitude: f32,
}

/// Decode each band, loop it forever, mix them, and block until the process is
/// interrupted (Ctrl-C). Bands at zero amplitude are skipped entirely.
pub fn play(bands: &[Band]) -> Result<()> {
    // The device sink prints a notice to stderr when dropped; playback only
    // ends when the whole process does, so that notice is just noise.
    let mut device = DeviceSinkBuilder::open_default_sink().context("open audio output device")?;
    device.log_on_drop(false);

    // Hold every player for the life of playback; dropping one stops its band.
    let mut players = Vec::new();
    for band in bands {
        if band.amplitude <= 0.0 {
            continue;
        }
        let file =
            File::open(&band.path).with_context(|| format!("open band {}", band.path.display()))?;
        // `Decoder` is not `Clone`, so `repeat_infinite` needs a `Buffered`
        // wrapper, which caches decoded samples and replays them each loop.
        let source = Decoder::new(BufReader::new(file))
            .with_context(|| format!("decode band {}", band.path.display()))?
            .buffered()
            .repeat_infinite()
            .amplify(band.amplitude);
        let player = Player::connect_new(device.mixer());
        player.append(source);
        players.push(player);
    }

    if players.is_empty() {
        bail!("every band is muted; nothing to play");
    }

    // The loops never end on their own; park forever (cheaper than a timed
    // sleep loop) until the user sends Ctrl-C, which terminates the process and
    // tears down the device sink. `park` may wake spuriously, so loop.
    loop {
        thread::park();
    }
}
