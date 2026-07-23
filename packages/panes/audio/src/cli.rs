//! CLI surface, in the compositor's style: long flags, doc comments as help
//! text. The defaults are the production values the guest image passes
//! explicitly (one source of truth for the numbers lives in
//! `guest-image/nixos.nix`, which must keep the `PipeWire` tap's format in sync
//! with `--rate`/`--channels`).

use std::path::PathBuf;

#[derive(Debug, clap::Parser)]
#[command(
    name = "panes-audio",
    about = "Ship the guest's PipeWire PCM mix to the macOS host over a byte stream"
)]
pub struct Cli {
    /// `AF_VSOCK` port to listen on: the production transport inside the VM
    /// (panes-host reaches it through libkrun's vsock port map). Ignored when
    /// --listen-unix or --listen-tcp is given.
    #[arg(long, value_name = "PORT", default_value_t = panes_protocol::audio::VSOCK_PORT)]
    pub listen_vsock: u32,

    /// Listen on a unix socket instead of vsock (off-VM development; a stale
    /// socket file at PATH is removed).
    #[arg(long, value_name = "PATH", conflicts_with = "listen_tcp")]
    pub listen_unix: Option<PathBuf>,

    /// Listen on TCP instead of vsock (off-VM development), e.g.
    /// "127.0.0.1:7102".
    #[arg(long, value_name = "ADDR")]
    pub listen_tcp: Option<String>,

    /// TCP address of `PipeWire`'s protocol-simple capture socket (raw PCM in
    /// the `--rate`/`--channels` format, s16le). TCP because the module's unix
    /// accept path is dead code in `PipeWire` 1.6 (`module-protocol-simple.c`
    /// rejects unix clients with `goto error`); loopback-bound in the guest.
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:7103")]
    pub pcm_tcp: String,

    /// Sample rate (Hz) the tap is configured to produce; advertised to the
    /// host in the audio Hello.
    #[arg(long, default_value_t = 48000)]
    pub rate: u32,

    /// Interleaved channel count the tap is configured to produce; advertised
    /// to the host in the audio Hello.
    #[arg(long, default_value_t = 2)]
    pub channels: u16,

    /// Log filter: a level ("info") or tracing directives
    /// (`info,panes_audio=debug`).
    #[arg(long, default_value = "info")]
    pub log_level: String,
}
