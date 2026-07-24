//! `vcs-prompt`: the version-control segment of the starship prompt, for jj
//! workspaces as well as git repositories.
//!
//! Starship's built-in `git_*` modules cannot be disabled per directory, and
//! inside a colocated jj repo they describe the exported git view: a detached
//! HEAD sitting wherever `jj git export` last wrote, with none of the working
//! copy's real state. So the config turns them off and calls this instead,
//! which picks the VCS by walking up from the prompt directory and renders one
//! segment for whichever it finds.
//!
//! ```text
//! $ vcs-prompt            # colocated jj fork checkout
//! on 󱗆 lsurukvy ix-patched+2 *
//! $ vcs-prompt            # plain git checkout
//! on  main !3?1⇡2
//! ```

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use color_eyre::eyre::{Result, WrapErr};

mod git;
mod jj;
mod render;
mod workspace;

use workspace::Workspace;

#[derive(Parser)]
#[command(
    version,
    about = "Starship VCS segment: jj working-copy state in a jj workspace, git branch and status elsewhere"
)]
struct Cli {
    /// Directory to describe. Defaults to the working directory, which is the
    /// directory starship is rendering the prompt for.
    #[arg(long, global = true)]
    cwd: Option<PathBuf>,
    /// Print the segment without ANSI escapes.
    #[arg(long, global = true)]
    no_color: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Exit 0 inside a jj or git workspace, 1 outside one. This is the
    /// `when` gate for the custom module: it only stats directories, so it
    /// costs nothing next to rendering.
    Detect,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cwd = match working_directory(&cli) {
        Ok(cwd) => cwd,
        Err(error) => return report(&error),
    };
    let Some(workspace) = workspace::discover(&cwd) else {
        // Outside a workspace there is no segment. Starship reads the
        // non-zero exit as "render nothing", which is both what `detect`
        // promises and what an unversioned directory should show.
        return ExitCode::FAILURE;
    };

    if matches!(cli.command, Some(Command::Detect)) {
        return ExitCode::SUCCESS;
    }

    match segment(&workspace, !cli.no_color) {
        Ok(segment) => {
            println!("{segment}");
            ExitCode::SUCCESS
        }
        Err(error) => report(&error),
    }
}

fn working_directory(cli: &Cli) -> Result<PathBuf> {
    cli.cwd.as_ref().map_or_else(
        || env::current_dir().wrap_err("failed to read the working directory"),
        |cwd| Ok(cwd.clone()),
    )
}

fn segment(workspace: &Workspace, color: bool) -> Result<String> {
    Ok(match workspace {
        Workspace::Jj(root) => render::jj(&jj::head(root)?, color),
        Workspace::Git(root) => render::git(&git::head(root)?, color),
    })
}

/// A prompt cannot pop up a backtrace: the report is one line on stderr, which
/// starship logs, and an empty segment.
fn report(error: &color_eyre::Report) -> ExitCode {
    eprintln!("vcs-prompt: {error:#}");
    ExitCode::FAILURE
}
