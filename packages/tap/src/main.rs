//! `tap`: a minimal terminal session manager for tiling-WM users.
//!
//! Detach and reattach to sessions without tmux's tiling layer, and share a
//! session with others (the screen sizes to the smallest attached client). See
//! the crate README for the design; the daemon/client split lives in
//! [`daemon`] and [`attach`].

mod attach;
mod client;
mod config;
mod daemon;
mod editor;
mod index;
mod input;
mod names;
mod term;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Terminal session manager for tiling-WM users.
#[derive(Parser)]
#[command(name = "tap", version, about = "Terminal session manager for tiling-WM users")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start a new session (the default when no subcommand is given).
    Start {
        /// Command to run; defaults to an interactive `$SHELL`.
        #[arg(trailing_var_arg = true)]
        command: Vec<String>,
        /// Start in the background without attaching.
        #[arg(short, long)]
        detached: bool,
        /// Use a specific session id instead of a generated one.
        #[arg(long)]
        id: Option<String>,
    },
    /// Attach to a running session (the most recent one if unspecified).
    Attach {
        /// Session id.
        session: Option<String>,
    },
    /// List active sessions.
    List,
    /// Print a session's screen as text.
    Scrollback {
        /// Session id (defaults to the most recent).
        #[arg(short, long)]
        session: Option<String>,
        /// Limit to the last N rows.
        #[arg(short, long)]
        lines: Option<usize>,
    },
    /// Print a session's cursor position.
    Cursor {
        /// Session id (defaults to the most recent).
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Print a session's negotiated size.
    Size {
        /// Session id (defaults to the most recent).
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Type text into a session without attaching.
    Inject {
        /// Session id (defaults to the most recent).
        #[arg(short, long)]
        session: Option<String>,
        /// Text to inject.
        text: String,
    },
    /// Stream a session's raw output to stdout.
    Subscribe {
        /// Session id (defaults to the most recent).
        #[arg(short, long)]
        session: Option<String>,
    },
    /// Terminate a session and its child process.
    Kill {
        /// Session id (defaults to the most recent).
        session: Option<String>,
    },
    /// Internal: run the session daemon. Spawned by `start`; not for direct use.
    #[command(hide = true)]
    Daemon {
        /// Session id.
        #[arg(long)]
        id: String,
        /// Socket path to bind.
        #[arg(long)]
        socket: PathBuf,
        /// Command to run, after `--`.
        #[arg(last = true)]
        command: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Command::Start {
        command: Vec::new(),
        detached: false,
        id: None,
    });

    match command {
        Command::Start { command, detached, id } => client::start(command, detached, id).await,
        Command::Attach { session } => attach::run(session).await,
        Command::List => {
            client::list();
            Ok(())
        }
        Command::Scrollback { session, lines } => client::scrollback(session, lines).await,
        Command::Cursor { session } => client::cursor(session).await,
        Command::Size { session } => client::size(session).await,
        Command::Inject { session, text } => client::inject(session, text).await,
        Command::Subscribe { session } => client::subscribe(session).await,
        Command::Kill { session } => client::kill(session).await,
        Command::Daemon { id, socket, command } => daemon::run(id, socket, command).await,
    }
}
