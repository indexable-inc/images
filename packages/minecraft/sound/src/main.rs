mod assets;
mod audio;

use std::env;
use std::io::Write as _;
use std::process::{Command, ExitCode, Stdio};

use clap::{Args, Parser, Subcommand};
use snafu::{ResultExt as _, Snafu};

use crate::assets::{AssetError, MinecraftAssets};
use crate::audio::{PlayError, PlaybackOptions};

#[derive(Parser)]
#[command(
    name = "minecraft-sound",
    about = "Play Minecraft sounds from the command line",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List available sounds, optionally filtered by a substring
    List { pattern: Option<String> },
    /// Play a sound by name (e.g. mob/zombie/death)
    Play(PlayArgs),
}

#[derive(Args)]
struct PlayArgs {
    /// Sound path (e.g. mob/zombie/death). If empty, does nothing; an unknown
    /// non-empty name is an error.
    sound: Option<String>,

    /// Wait for playback to finish instead of returning immediately
    #[arg(short, long)]
    wait: bool,

    /// Playback pitch (Minecraft-style: shifts pitch and tempo together).
    /// Clamped to [0.5, 2.0]; 1.0 is normal.
    #[arg(long, default_value_t = 1.0)]
    pitch: f32,

    /// Linear volume multiplier; 1.0 is normal, 0.0 is silent.
    #[arg(long, default_value_t = 1.0)]
    volume: f32,

    /// Internal: run playback in the foreground (used by the background spawn)
    #[arg(long, hide = true)]
    foreground: bool,
}

/// Top-level CLI error, wrapping the asset and playback error domains plus the
/// process plumbing used by the detached background spawn.
#[derive(Debug, Snafu)]
enum CliError {
    #[snafu(display("{source}"), context(false))]
    Assets { source: AssetError },

    #[snafu(display("{source}"), context(false))]
    Playback { source: PlayError },

    #[snafu(display("Failed to find current executable"))]
    CurrentExe { source: std::io::Error },

    #[snafu(display("Failed to spawn background playback process"))]
    Spawn { source: std::io::Error },
}

#[expect(
    clippy::print_stderr,
    reason = "top-level CLI error reporting before exit"
)]
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), CliError> {
    let cli = Cli::parse();

    match cli.command {
        Commands::List { pattern } => list(pattern.as_deref()),
        Commands::Play(args) => play(args),
    }
}

fn list(pattern: Option<&str>) -> Result<(), CliError> {
    let assets = MinecraftAssets::load()?;
    let mut out = std::io::stdout().lock();
    for sound in assets.list_sounds(pattern)? {
        // Stop quietly when the reader goes away (e.g. `| head`) instead of
        // erroring on a broken pipe.
        if writeln!(out, "{sound}").is_err() {
            break;
        }
    }
    Ok(())
}

fn play(args: PlayArgs) -> Result<(), CliError> {
    // Skip silently when no sound is given, so hooks can pass an empty string
    // to disable a particular event. A non-empty unknown name is an error.
    let Some(sound) = args.sound.filter(|sound| !sound.is_empty()) else {
        return Ok(());
    };

    let options = PlaybackOptions {
        pitch: args.pitch,
        volume: args.volume,
    };

    // Resolve the sound up front in every mode. This is the fix for the
    // silent-failure footgun: the old default (non-`--wait`) path spawned a
    // detached child that swallowed all errors, so a typo'd name exited 0 and
    // played nothing. Resolving here means a bad name fails loudly before we
    // ever spawn.
    let assets = MinecraftAssets::load()?;
    let path = assets.resolve_sound(&sound)?;

    if args.wait || args.foreground {
        audio::play_ogg(&path, options)?;
    } else {
        // Re-spawn ourselves detached, in `--foreground` `--wait` mode, so the
        // caller returns immediately while the sound keeps playing. The name is
        // already validated, so this only fails on real spawn errors.
        let exe = env::current_exe().context(CurrentExeSnafu)?;
        let mut command = Command::new(exe);
        command
            .args(["play", "--foreground", "--wait"])
            .args(["--pitch", &options.pitch.to_string()])
            .args(["--volume", &options.volume.to_string()])
            .arg(&sound)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().context(SpawnSnafu)?;
    }

    Ok(())
}
