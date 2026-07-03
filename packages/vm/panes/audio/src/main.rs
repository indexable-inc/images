//! Guest audio pump: `PipeWire`'s protocol-simple PCM tap -> `panes-host` over
//! vsock port 7102 (or a unix/TCP socket for off-VM development). The wire
//! contract lives in `panes-protocol`'s `audio` module; the `PipeWire` side
//! (null sink + protocol-simple tap) is configured by
//! `packages/vm/panes/guest-image/nixos.nix`. See index#1686.

mod cli;
mod serve;

use clap::Parser as _;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    // EnvFilter accepts full tracing directives, so `--log-level` takes a bare
    // level ("debug") or a filter string ("info,panes_audio=debug").
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_new(&cli.log_level)?)
        .init();

    // The pump divides by the sample-frame size (channels * 2 bytes); a zero
    // would panic deep in the loop instead of failing legibly here.
    anyhow::ensure!(cli.channels >= 1, "--channels must be at least 1");
    anyhow::ensure!(cli.rate >= 1, "--rate must be at least 1");

    let listen = if let Some(path) = cli.listen_unix {
        serve::ListenSpec::Unix(path)
    } else if let Some(addr) = cli.listen_tcp {
        serve::ListenSpec::Tcp(addr)
    } else {
        vsock_listen_spec(cli.listen_vsock)?
    };
    serve::run(
        &listen,
        &cli.pcm_tcp,
        &serve::StreamFormat { rate: cli.rate, channels: cli.channels },
    )
}

#[cfg(target_os = "linux")]
fn vsock_listen_spec(port: u32) -> anyhow::Result<serve::ListenSpec> {
    Ok(serve::ListenSpec::Vsock(port))
}

/// The daemon has no host-side role (playback lives in panes-host); this stub
/// keeps `cargo test` for the portable pump/handshake logic working on
/// non-Linux development hosts, where only the unix/TCP listeners exist.
#[cfg(not(target_os = "linux"))]
fn vsock_listen_spec(_port: u32) -> anyhow::Result<serve::ListenSpec> {
    anyhow::bail!("AF_VSOCK is Linux-only; use --listen-unix or --listen-tcp on a development host")
}
