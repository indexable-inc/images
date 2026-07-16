//! `shared-audio`: play together, send no audio.
//!
//! One binary, three roles: the daemon that joins the session and plays,
//! thin clients that drive it over the control socket, and a macOS
//! menu-bar tray for local volume.

mod client;
mod control;
mod daemon;
mod tray;

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use base64::Engine as _;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "shared-audio", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Join the session and play; run this under launchd/systemd.
    Daemon(daemon::Opts),
    /// Show daemon, clock, and score state.
    Status,
    /// Adjust this machine's local volume (never shared).
    Volume {
        #[command(subcommand)]
        action: VolumeAction,
    },
    /// Publish a WASM (or WAT) instrument module to every peer.
    Publish {
        /// Path to a `.wasm` or `.wat` module implementing the sa ABI.
        path: PathBuf,
        /// Shared frame to switch at; defaults to one second from now.
        #[arg(long)]
        at: Option<u64>,
    },
    /// Set a shared instrument control for everyone.
    SetControl { control: u16, value: f32 },
    /// Schedule a shared control change at an exact shared frame.
    Schedule {
        at_frame: u64,
        control: u16,
        value: f32,
    },
    /// macOS menu-bar volume item (talks to the local daemon).
    Tray,
}

#[derive(Subcommand)]
enum VolumeAction {
    /// Raise local volume a step.
    Up,
    /// Lower local volume a step.
    Down,
    /// Set local gain (0.0..=2.0).
    Set { gain: f32 },
    /// Silence local output (the session keeps playing).
    Mute,
    /// Restore local output.
    Unmute,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match Cli::parse().command {
        Command::Daemon(opts) => daemon::run(opts),
        Command::Status => client::run(&control::Request::Status),
        Command::Volume { action } => {
            let request = match action {
                VolumeAction::Up => volume(None, Some(0.1), None),
                VolumeAction::Down => volume(None, Some(-0.1), None),
                VolumeAction::Set { gain } => volume(Some(gain), None, None),
                VolumeAction::Mute => volume(None, None, Some(true)),
                VolumeAction::Unmute => volume(None, None, Some(false)),
            };
            client::run(&request)
        }
        Command::Publish { path, at } => {
            let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            client::run(&control::Request::Publish {
                wasm_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                at_frame: at,
            })
        }
        Command::SetControl { control, value } => {
            client::run(&control::Request::SetControl { control, value })
        }
        Command::Schedule {
            at_frame,
            control,
            value,
        } => client::run(&control::Request::Schedule {
            at_frame,
            control,
            value,
        }),
        Command::Tray => tray::run(),
    }
}

const fn volume(set: Option<f32>, step: Option<f32>, muted: Option<bool>) -> control::Request {
    control::Request::Volume { set, step, muted }
}
