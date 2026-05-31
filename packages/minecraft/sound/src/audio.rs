use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use rodio::source::Source as _;
use rodio::{Decoder, OutputStream, Sink};
use snafu::{ResultExt as _, Snafu};

/// Minimum playback pitch/speed factor, matching Minecraft's clamp.
pub const MIN_PITCH: f32 = 0.5;
/// Maximum playback pitch/speed factor, matching Minecraft's clamp.
pub const MAX_PITCH: f32 = 2.0;

/// Errors that can occur while decoding or playing a sound file.
#[derive(Debug, Snafu)]
pub enum PlayError {
    #[snafu(display("Failed to open audio output device"))]
    OpenDevice { source: rodio::StreamError },

    #[snafu(display("Failed to open sound file: {}", path.display()))]
    OpenFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to decode OGG file: {}", path.display()))]
    Decode {
        path: PathBuf,
        source: rodio::decoder::DecoderError,
    },

    #[snafu(display("Failed to create audio sink"))]
    CreateSink { source: rodio::PlayError },
}

/// How to play a sound: Minecraft-style pitch and a linear volume multiplier.
#[derive(Debug, Clone, Copy)]
pub struct PlaybackOptions {
    /// Playback speed factor. Like Minecraft, this shifts pitch and tempo
    /// together (no resampling). Clamped to `[MIN_PITCH, MAX_PITCH]`.
    pub pitch: f32,
    /// Linear amplitude multiplier; `1.0` is unchanged. Clamped to `>= 0.0`.
    pub volume: f32,
}

impl Default for PlaybackOptions {
    fn default() -> Self {
        Self {
            pitch: 1.0,
            volume: 1.0,
        }
    }
}

impl PlaybackOptions {
    /// Pitch clamped to Minecraft's supported `[0.5, 2.0]` range.
    const fn clamped_pitch(self) -> f32 {
        self.pitch.clamp(MIN_PITCH, MAX_PITCH)
    }

    /// Volume clamped to be non-negative; a negative amplitude is meaningless.
    const fn clamped_volume(self) -> f32 {
        self.volume.max(0.0)
    }
}

/// Decode and play an OGG/Vorbis file, blocking until playback finishes.
///
/// `pitch` shifts speed and pitch together (Minecraft semantics) and `volume`
/// scales amplitude; both are clamped to sane ranges before use.
///
/// # Errors
/// Returns an error if the audio output device cannot be opened, the file
/// cannot be read, or the stream cannot be decoded.
pub fn play_ogg(path: &Path, options: PlaybackOptions) -> Result<(), PlayError> {
    let (_stream, handle) = OutputStream::try_default().context(OpenDeviceSnafu)?;

    let file = File::open(path).context(OpenFileSnafu { path })?;
    let source = Decoder::new(BufReader::new(file)).context(DecodeSnafu { path })?;

    let sink = Sink::try_new(&handle).context(CreateSinkSnafu)?;
    // `speed` changes play rate without resampling, so pitch and tempo move
    // together — the same effect Minecraft's pitch parameter produces.
    let source = source
        .speed(options.clamped_pitch())
        .amplify(options.clamped_volume());
    sink.append(source);
    sink.sleep_until_end();

    Ok(())
}
