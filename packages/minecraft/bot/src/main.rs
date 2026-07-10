//! mc-bot: a headless Minecraft client that joins a server and records the
//! session as a `ReplayMod` `.mcpr` replay.
//!
//! The bot logs in (offline mode), walks the configuration handshake, then
//! sits in the play state answering keep-alives while every clientbound
//! packet is captured. The result opens in `ReplayMod`, so an integration test
//! that drives a live server can export exactly what a client saw — when a
//! test fails, the replay is the client-side trace to scrub through
//! (tests/minestom-spleef-vm.nix records the spleef server this way).
//!
//! The wire primitives (framing, `VarInt`s, strings) come from mc-protocol
//! (packages/minecraft/protocol), the same crate under mc-probe's
//! Python bindings and mc-probe-kt's JVM bindings; this binary adds only the
//! state machine and the `.mcpr` container.

mod framing;
mod mcpr;
mod packets;
mod session;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use mc_protocol::ServerAddress;

use crate::mcpr::{Recorder, ReplayInfo};

#[derive(Debug, Parser)]
#[command(name = "mc-bot")]
#[command(about = "Join a Minecraft server and record the session as a ReplayMod .mcpr replay")]
struct Args {
    /// Server to join, as host[:port] (port defaults to 25565).
    address: String,

    /// Username to log in with (offline mode; the profile id is derived the
    /// way vanilla derives offline ids).
    #[arg(long, default_value = "replay-bot")]
    username: String,

    /// Protocol version for the handshake. Must match the server's — kept
    /// explicit at the call site, like the probes' --protocol-version, so
    /// version bumps are deliberate.
    #[arg(long)]
    protocol_version: i32,

    /// Version display name written to the replay's metaData.json
    /// ("mcversion"), e.g. 26.1.2; `ReplayMod` shows it in the replay list.
    #[arg(long)]
    mc_version: String,

    /// How long to keep recording after reaching the play state.
    #[arg(long, default_value_t = 10)]
    record_seconds: u64,

    /// Network timeout in seconds for connect and each read/write.
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Where to write the .mcpr archive.
    #[arg(long)]
    output: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("mc-bot: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> anyhow::Result<()> {
    let address = ServerAddress::parse(&args.address)?;
    let config = session::Config {
        username: args.username.clone(),
        protocol_version: args.protocol_version,
        record_for: Duration::from_secs(args.record_seconds),
        timeout: Duration::from_secs(args.timeout),
    };

    let mut recorder = Recorder::new();
    let session = session::run(&address, &config, &mut recorder);

    // The replay is the debugging artifact: write whatever was captured even
    // when the session ended in a kick or network failure, so a failing e2e
    // run still leaves the client-side trace behind.
    if !recorder.is_empty() {
        let info = ReplayInfo {
            server_name: format!("{}:{}", address.host, address.port),
            mc_version: args.mc_version.clone(),
            protocol_version: args.protocol_version,
        };
        recorder
            .write(&args.output, &info)
            .with_context(|| format!("writing {}", args.output.display()))?;
        println!(
            "mc-bot: recorded {} packets over {}ms into {}",
            recorder.packets(),
            recorder.duration().as_millis(),
            args.output.display(),
        );
    }
    session
}
