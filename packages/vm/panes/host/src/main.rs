//! macOS host agent: presents guest-Linux toplevels as native `NSWindow`s and
//! forwards input back to the guest compositor. See index#1686 and
//! `panes-protocol` for the wire contract.
//!
//! Process shape: the `AppKit` main thread owns every window and all state
//! (`app::APP` is a main-thread `thread_local`); a supervisor thread owns the
//! socket, reconnects with backoff, and `dispatch_async`s decoded [`ToHost`]
//! messages onto the main queue; a writer thread drains outgoing [`ToGuest`]
//! messages so the main thread never blocks on the socket.
//!
//! [`ToHost`]: panes_protocol::ToHost
//! [`ToGuest`]: panes_protocol::ToGuest

mod mock;

#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod conn;
#[cfg(target_os = "macos")]
mod keymap;
#[cfg(target_os = "macos")]
mod render;
#[cfg(target_os = "macos")]
mod view;
#[cfg(target_os = "macos")]
mod window;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Present guest-Linux windows as native macOS windows")]
struct Cli {
    /// Unix socket to connect to (the guest side of the vsock port map).
    #[arg(long, value_name = "PATH")]
    connect: Option<PathBuf>,

    /// TCP address to connect to instead of a unix socket (debugging).
    #[arg(long, value_name = "ADDR", conflicts_with = "connect")]
    tcp: Option<String>,

    /// Prefix prepended to every window title.
    #[arg(long, value_name = "PREFIX", default_value = "")]
    title_prefix: String,

    /// Serve the built-in mock guest on a temp socket and connect to it:
    /// one animated test window, received input logged to stderr.
    #[arg(long, conflicts_with_all = ["connect", "tcp"])]
    mock: bool,

    /// Only serve the mock guest on PATH and block (headless; works on any
    /// OS). Point a separately-launched `panes-host --connect PATH` at it.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["connect", "tcp", "mock"])]
    mock_serve: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(path) = cli.mock_serve {
        return match mock::serve(&path) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("panes-host: mock guest failed: {error}");
                ExitCode::FAILURE
            }
        };
    }

    run_host(cli)
}

#[cfg(target_os = "macos")]
fn run_host(cli: Cli) -> ExitCode {
    let target = if cli.mock {
        let path = std::env::temp_dir().join(format!("panes-host-mock-{}.sock", std::process::id()));
        let serve_path = path.clone();
        std::thread::spawn(move || {
            if let Err(error) = mock::serve(&serve_path) {
                eprintln!("panes-host: mock guest failed: {error}");
                std::process::exit(1);
            }
        });
        conn::Target::Unix(path)
    } else if let Some(path) = cli.connect {
        conn::Target::Unix(path)
    } else if let Some(addr) = cli.tcp {
        conn::Target::Tcp(addr)
    } else {
        eprintln!("panes-host: one of --connect, --tcp, --mock, --mock-serve is required");
        return ExitCode::FAILURE;
    };

    app::run(target, cli.title_prefix)
}

#[cfg(not(target_os = "macos"))]
fn run_host(_cli: Cli) -> ExitCode {
    eprintln!("panes-host: the window agent requires macOS (only --mock-serve works here)");
    ExitCode::FAILURE
}
