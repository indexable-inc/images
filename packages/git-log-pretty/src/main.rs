//! `git-log-pretty`: a pretty `git log` viewer.
//!
//! With no subcommand it lists the commits HEAD is ahead of `main`, newest
//! first, each as a one-line summary plus an icon tree of the files it touched.
//! The `diff` subcommand renders just the changed-file tree between two refs.

use anstyle::{AnsiColor, Color};
use clap::{Parser, Subcommand};
use color_eyre::eyre::{Result, WrapErr};

mod display;
mod git;
mod palette;
mod time;
mod tree;

use palette::{Theme, fg, paint};

/// How many ahead-of-base commits to print before summarizing the rest. The
/// list is newest-first, so the cap keeps the common "what have I done lately"
/// view short.
const MAX_COMMITS: usize = 15;

#[derive(Parser)]
#[command(name = "git-log-pretty", about = "A pretty git log viewer with file-icon trees")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show the changed-file tree between two refs, like `git diff --stat` with
    /// icons.
    Diff {
        /// Base ref to compare against.
        #[arg(default_value = "main")]
        base: String,
        /// Head ref to compare; defaults to the current HEAD.
        #[arg(default_value = "HEAD")]
        head: String,
    },
}

fn main() -> Result<()> {
    color_eyre::install()?;

    match Cli::parse().command {
        Some(Command::Diff { base, head }) => run_diff(&base, &head).wrap_err("failed to render diff stats"),
        None => run_log().wrap_err("failed to render git log"),
    }
}

/// Render the ahead-of-`main` commit log for the current repository.
fn run_log() -> Result<()> {
    let repo = git::discover()?;
    let ahead = git::commits_ahead(&repo, "main")?;

    if ahead.is_empty() {
        println!("{}", paint(fg(Color::Ansi(AnsiColor::Green)), "All caught up with main"));
        return Ok(());
    }

    let theme = Theme::detect();
    let hidden = ahead.len().saturating_sub(MAX_COMMITS);

    let header = if hidden > 0 {
        let detail = paint(
            fg(Color::Ansi(AnsiColor::BrightBlack)),
            &format!(" (showing first {MAX_COMMITS}, {hidden} more hidden)"),
        );
        format!("{count} commits ahead of main{detail}", count = ahead.len())
    } else {
        let label = if ahead.len() == 1 { "commit" } else { "commits" };
        format!("{count} {label} ahead of main", count = ahead.len())
    };
    println!("{}\n", paint(fg(Color::Ansi(AnsiColor::Cyan)), &header));

    for commit in ahead.iter().take(MAX_COMMITS) {
        display::print_commit(commit, theme)?;
    }

    Ok(())
}

/// Render the changed-file tree between `base` and `head`.
fn run_diff(base: &str, head: &str) -> Result<()> {
    let repo = git::discover()?;
    let files = git::diff_stat_files(&repo, base, head)?;

    if files.is_empty() {
        println!("{}", paint(fg(Color::Ansi(AnsiColor::Green)), "No changes found"));
        return Ok(());
    }

    let theme = Theme::detect();
    let header = format!(
        "{count} files changed in {base}...{head}",
        count = files.len(),
    );
    println!("{}\n", paint(fg(Color::Ansi(AnsiColor::Cyan)), &header));

    println!("{}", tree::render(&files, theme));
    println!();

    Ok(())
}
