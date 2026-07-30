//! index-delta: drift tracker for Nix-declared mutable files.
//!
//! Nix declares a file's content (the *base*); the file on disk stays a
//! plain writable file (the *upper*). Activation seeds the upper from the
//! base and gates every later switch: ephemeral files (the default) are
//! reseeded with their drift archived to the journal, durable files keep
//! your edits and *stage* an incoming base change as a conflict instead of
//! merging. `status --json` is the resolution queue — logical, format-aware
//! diffs in both directions, geared toward a model deciding what to absorb
//! into the Nix config (`apply-ops`), discard, adopt, or snooze.

mod apply;
mod cmd;
mod diff;
mod store;
mod tui;
mod value;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::store::Store;
use crate::value::Format;

#[derive(Parser)]
#[command(name = "index-delta", version, about)]
struct Cli {
    /// State directory (default: `$INDEX_DELTA_STATE_DIR`, then
    /// `$XDG_STATE_HOME/index-delta`, then `~/.local/state/index-delta`).
    #[arg(long, global = true, value_name = "DIR")]
    state_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Reconcile every file declared in a manifest (called by the nix
    /// module's activation step). Ephemeral files are reseeded, durable
    /// files fast-forward when clean and stage conflicts when not.
    Activate {
        /// Manifest JSON: {"files": [{"path", "source", "format"?,
        /// "persistence"?, "declaredAt"?, "sourceFile"?}]}.
        #[arg(long, value_name = "FILE")]
        manifest: PathBuf,
    },
    /// Rewrite every ephemeral file from its recorded base, archiving any
    /// drift to the journal first (called by the login agent; needs no
    /// manifest).
    Reseed,
    /// The resolution queue: every managed file's state with logical diffs
    /// in both directions.
    Status {
        /// Emit the machine contract instead of the human summary.
        #[arg(long)]
        json: bool,
        /// Exit 1 when anything is drifted or in conflict (for scripts).
        #[arg(long)]
        check: bool,
    },
    /// Browse mutable-file drift interactively.
    Tui,
    /// One file's logical diff (upper vs base).
    Diff {
        path: String,
        /// Unified text diff instead of logical ops.
        #[arg(long)]
        raw: bool,
    },
    /// Drop your edits: the (staged, if any) base wins.
    Discard { path: String },
    /// Accept the staged base as the new base but keep your edits — the
    /// conflict becomes plain drift.
    Adopt { path: String },
    /// Apply logical ops onto a file, format-preserving where the format
    /// allows. The mechanical half of absorbing drift into the repo.
    ApplyOps {
        /// File to edit (typically the repo copy of a config).
        file: String,
        /// Ops JSON (array of {op, path, ...}); `-` reads stdin.
        ops: String,
        /// Override format detection.
        #[arg(long, value_enum)]
        format: Option<Format>,
    },
    /// Silence a drifted file until its diff changes.
    Snooze { path: String },
    /// Archived state transitions — including the logical diffs of edits
    /// wiped by past ephemeral reseeds.
    Journal {
        path: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

fn run(cli: Cli) -> Result<ExitCode> {
    let store = Store::open(Store::resolve_root(cli.state_dir)?)?;
    match cli.command {
        Command::Activate { manifest } => cmd::activate(&store, &manifest)?,
        Command::Reseed => cmd::reseed_ephemeral(&store)?,
        Command::Status { json, check } => {
            cmd::status(&store, json)?;
            if check && cmd::pending_count(&store)? > 0 {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Tui => tui::run(&store)?,
        Command::Diff { path, raw } => cmd::print_diff(&store, &path, raw)?,
        Command::Discard { path } => cmd::discard(&store, &path)?,
        Command::Adopt { path } => cmd::adopt(&store, &path)?,
        Command::ApplyOps { file, ops, format } => cmd::apply_ops(&file, &ops, format)?,
        Command::Snooze { path } => cmd::snooze(&store, &path)?,
        Command::Journal { path, json } => cmd::journal(&store, path.as_deref(), json)?,
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    cli_entry::run("index-delta", run)
}
