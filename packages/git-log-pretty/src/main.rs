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

use std::collections::HashSet;
use std::io::{IsTerminal, Write};

use anstyle::{AnsiColor, Color};
use clap::{Parser, Subcommand};
use color_eyre::eyre::{Result, WrapErr};

mod avatar;
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

/// Avatar size to request from GitHub, in pixels. Larger than any cell box so
/// the terminal downscales rather than upscales.
const AVATAR_SIZE_PX: u32 = 128;

#[derive(Parser)]
#[command(name = "git-log-pretty", about = "A pretty git log viewer with file-icon trees")]
struct Cli {
    /// Write directly to stdout instead of piping through a pager.
    #[arg(long, global = true)]
    no_pager: bool,
    /// Don't draw author GitHub avatars inline (kitty graphics protocol).
    #[arg(long, global = true)]
    no_avatar: bool,
    /// Height of each inline avatar in terminal rows (0 disables avatars).
    #[arg(long, global = true, default_value_t = 2)]
    avatar_rows: u32,
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
    let want_avatars = !cli.no_avatar;
    let avatar_rows = cli.avatar_rows;
    match cli.command {
        Some(Command::Diff { base, head }) => {
            run_diff(&base, &head, allow_pager).wrap_err("failed to render diff stats")
        }
        None => {
            run_log(allow_pager, want_avatars, avatar_rows).wrap_err("failed to render git log")
        }
    }
}

/// Render the default commit log for the current repository. On `main` this is
/// `main`'s own recent history; on any other branch it is the commits HEAD is
/// ahead of `main`.
fn run_log(allow_pager: bool, want_avatars: bool, avatar_rows: u32) -> Result<()> {
    let repo = git::discover()?;
    let theme = detect();

    // On `main` there is nothing to be ahead of, so an ahead-of-main diff would
    // always be empty. Show recent history instead of "All caught up".
    let (header, commits) = if git::head_branch_name(&repo).as_deref() == Some("main") {
        ("Recent commits on main".to_string(), git::recent_commits(&repo, MAX_COMMITS)?)
    } else {
        let mut ahead = git::commits_ahead(&repo, "main")?;
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

        ahead.truncate(MAX_COMMITS);
        (header, ahead)
    };

    emit_log(&repo, &header, &commits, theme, allow_pager, want_avatars, avatar_rows)
}

/// Render the header and commit blocks, paging like `git log`.
///
/// When avatars are enabled (a graphics terminal, a real TTY, and not opted
/// out), each unique author image is transmitted to the terminal once, up
/// front, as a kitty Unicode-placeholder virtual placement. The paged text then
/// only carries placeholder cells, which are ordinary characters, so the log
/// pages and scrolls with the avatars in place instead of bypassing the pager.
fn emit_log(
    repo: &git2::Repository,
    header: &str,
    commits: &[git::AheadCommit<'_>],
    theme: Theme,
    allow_pager: bool,
    want_avatars: bool,
    avatar_rows: u32,
) -> Result<()> {
    let avatars_enabled = want_avatars
        && avatar_rows > 0
        && kitty::is_supported()
        && std::io::stdout().is_terminal();

    // A fetch or runtime-build failure shouldn't sink the whole log; fall back
    // to the plain, still-paged renderer.
    let fetched = avatars_enabled.then(|| fetch_avatars(repo, commits).ok()).flatten();

    // Transmit the pixels before the pager starts drawing, so the placeholder
    // cells it later prints have an image to resolve against.
    if let Some(fetched) = &fetched {
        transmit_avatars(fetched, avatar_rows)?;
    }

    pager::paged(allow_pager, |out| {
        writeln!(out, "{}\n", paint(fg(Color::Ansi(AnsiColor::Cyan)), header))?;
        for (index, commit) in commits.iter().enumerate() {
            match fetched.as_ref().and_then(|fetched| fetched.get(index)) {
                Some(avatar) => {
                    display::print_commit_with_avatar(out, commit, theme, avatar.as_ref(), avatar_rows)?;
                }
                None => display::print_commit(out, commit, theme)?,
            }
        }
        Ok(())
    })
}

/// Transmit each unique avatar's pixels to the terminal as a kitty virtual
/// placement, sized to the avatar box. Writes straight to stdout (the TTY) and
/// flushes, so the images are stored before the pager prints any placeholders.
fn transmit_avatars(fetched: &[Option<avatar::Avatar>], rows: u32) -> Result<()> {
    let cols = display::avatar_cols(rows);
    let mut out = std::io::stdout().lock();
    let mut sent = HashSet::new();
    for avatar in fetched.iter().flatten() {
        if sent.insert(avatar.id) {
            let sequence = kitty::transmit_virtual(&kitty::Image::Png(&avatar.png), avatar.id, cols, rows);
            out.write_all(sequence.as_bytes())?;
        }
    }
    out.flush().wrap_err("failed to flush avatar images to the terminal")
}

/// Resolve and download each commit author's avatar, one slot per commit.
///
/// `None` marks an author that could not be resolved. The async `github-avatar`
/// client is driven on a short-lived current-thread runtime.
fn fetch_avatars(
    repo: &git2::Repository,
    commits: &[git::AheadCommit<'_>],
) -> Result<Vec<Option<avatar::Avatar>>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .wrap_err("failed to build the avatar fetch runtime")?;
    let mut resolver = avatar::Resolver::new(repo, AVATAR_SIZE_PX);
    let fetched = runtime.block_on(async {
        let mut fetched = Vec::with_capacity(commits.len());
        for ahead in commits {
            let email = ahead.commit.author().email().unwrap_or_default().to_string();
            let sha = ahead.commit.id().to_string();
            fetched.push(resolver.avatar_for(&email, &sha).await);
        }
        fetched
    });
    Ok(fetched)
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
