//! Guest-side headless Wayland compositor: exports each `xdg_toplevel` over
//! vsock (or a unix/TCP socket for off-VM development) to `panes-host` on the
//! macOS side. The wire contract lives in `packages/vm/panes/protocol`; the
//! design constraints (ack-paced frame callbacks, damage tiles, host-side
//! resize) are documented there and in index#1686.

mod cli;
// The pure damage/tile logic compiles everywhere so its unit tests run on any
// development host; outside Linux nothing calls it, hence the dead_code allow.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod frame;

#[cfg(target_os = "linux")]
mod compositor;

use clap::Parser as _;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    // EnvFilter accepts full tracing directives, so `--log-level` takes a bare
    // level ("debug") or a filter string ("info,smithay=warn").
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_new(&cli.log_level)?)
        .init();
    run(&cli)
}

#[cfg(target_os = "linux")]
fn run(cli: &cli::Cli) -> anyhow::Result<()> {
    compositor::run(cli)
}

// smithay (and AF_VSOCK) are Linux-only; this binary has no host-side role,
// panes-host is the macOS half. The stub keeps `cargo test` for the portable
// frame logic working on non-Linux development hosts.
#[cfg(not(target_os = "linux"))]
fn run(_cli: &cli::Cli) -> anyhow::Result<()> {
    anyhow::bail!(
        "panes-compositor runs inside a Linux guest only (see packages/vm/panes/host for the macOS side)"
    )
}
