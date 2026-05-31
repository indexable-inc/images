//! `git-log-pretty`: a pretty `git log` viewer.
//!
//! With no subcommand it shows recent history newest-first, each commit a
//! one-line summary plus an icon tree of the files it touched. On `main` it
//! lists `main`'s own recent commits; on any other branch it lists only the
//! commits HEAD is ahead of `main`. The `diff` subcommand renders just the
//! changed-file tree between two refs.
//!
//! On a terminal the output is piped through a pager (`$PAGER`, else `less`),
//! like `git log`; redirected output skips the pager. See [`pager`].

use std::io::Write;

use anstyle::{AnsiColor, Color};
use clap::{Parser, Subcommand};
use color_eyre::eyre::{Result, WrapErr};

mod display;
mod git;
mod pager;
mod palette;
mod time;
mod tree;

use palette::{Theme, detect, fg, paint};

/// How many ahead-of-base commits to print before summarizing the rest. The
/// list is newest-first, so the cap keeps the common "what have I done lately"
/// view short.
const MAX_COMMITS: usize = 15;

#[derive(Parser)]
#[command(name = "git-log-pretty", about = "A pretty git log viewer with file-icon trees")]
struct Cli {
    /// Write directly to stdout instead of piping through a pager.
    #[arg(long, global = true)]
    no_pager: bool,
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

    let cli = Cli::parse();
    let allow_pager = !cli.no_pager;
    match cli.command {
        Some(Command::Diff { base, head }) => {
            run_diff(&base, &head, allow_pager).wrap_err("failed to render diff stats")
        }
        None => run_log(allow_pager).wrap_err("failed to render git log"),
    }
}

/// Render the default commit log for the current repository. On `main` this is
/// `main`'s own recent history; on any other branch it is the commits HEAD is
/// ahead of `main`.
fn run_log(allow_pager: bool) -> Result<()> {
    let repo = git::discover()?;
    let theme = detect();

    // On `main` there is nothing to be ahead of, so an ahead-of-main diff would
    // always be empty. Show recent history instead of "All caught up".
    if git::head_branch_name(&repo).as_deref() == Some("main") {
        let recent = git::recent_commits(&repo, MAX_COMMITS)?;
        return pager::paged(allow_pager, |out| {
            print_log(out, "Recent commits on main", &recent, theme)
        });
    }

    let ahead = git::commits_ahead(&repo, "main")?;
    if ahead.is_empty() {
        println!("{}", paint(fg(Color::Ansi(AnsiColor::Green)), "All caught up with main"));
        return Ok(());
    }

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

    let shown = &ahead[..ahead.len().min(MAX_COMMITS)];
    pager::paged(allow_pager, |out| print_log(out, &header, shown, theme))
}

/// Write a cyan header followed by each commit block to `out`.
fn print_log(out: &mut dyn Write, header: &str, commits: &[git::AheadCommit<'_>], theme: Theme) -> Result<()> {
    writeln!(out, "{}\n", paint(fg(Color::Ansi(AnsiColor::Cyan)), header))?;
    for commit in commits {
        display::print_commit(out, commit, theme)?;
    }
    Ok(())
}

/// Render the changed-file tree between `base` and `head`.
fn run_diff(base: &str, head: &str, allow_pager: bool) -> Result<()> {
    let repo = git::discover()?;
    let files = git::diff_stat_files(&repo, base, head)?;

    if files.is_empty() {
        println!("{}", paint(fg(Color::Ansi(AnsiColor::Green)), "No changes found"));
        return Ok(());
    }

    let theme = detect();
    let header = format!(
        "{count} files changed in {base}...{head}",
        count = files.len(),
    );

    pager::paged(allow_pager, |out| {
        writeln!(out, "{}\n", paint(fg(Color::Ansi(AnsiColor::Cyan)), &header))?;
        writeln!(out, "{}", tree::render(&files, theme))?;
        writeln!(out)?;
        Ok(())
    })
}
