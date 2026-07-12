//! `prs`: one view of every patch this repo carries against a vendored
//! dependency, and the upstream PR each patch corresponds to.
//!
//! Patches are discovered from the fork registry (`lib/fork-packages.nix`),
//! each series' tool-owned `upstream-status.json`, PR URLs in the patch
//! headers themselves, and loose `*.patch` files anywhere else in the tree;
//! see [`discover`]. Live PR state (open/draft/merged/closed, CI rollup,
//! review decision, unresolved review threads) comes from one batched GitHub
//! GraphQL query; without a token the tool still lists every patch and says
//! why the status columns are empty.
//!
//! On a terminal it runs the [`tui`]; piped output (or `--plain`) prints one
//! aligned table.

use std::io::IsTerminal;
use std::path::PathBuf;

use clap::Parser;
use color_eyre::eyre::Result;

mod discover;
mod github;
mod model;
mod plain;
mod tui;

#[derive(Parser)]
#[command(
    name = "prs",
    about = "View the repo's vendored-dependency patches and their upstream PR status",
    after_help = "TUI keys:\n  \
        j/k move, gg/G jump, / filter, Enter/o open the PR in the browser,\n  \
        e edit the patch in $EDITOR, E open its directory, d preview the diff,\n  \
        y copy the PR URL (OSC 52), r refresh PR status, ? help, q quit"
)]
struct Cli {
    /// Print a plain table instead of the interactive TUI.
    #[arg(long)]
    plain: bool,
    /// Skip the GitHub API; list patches and PR links only.
    #[arg(long)]
    offline: bool,
    /// Fork mapping JSON (rendered `lib/fork-packages.nix`); defaults to
    /// `PRS_FORK_MAPPING`, else `nix eval` on the repo's registry.
    #[arg(long)]
    mapping: Option<PathBuf>,
    /// Repo root; defaults to the nearest ancestor with lib/fork-packages.nix.
    #[arg(long)]
    repo: Option<PathBuf>,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let root = discover::repo_root(cli.repo);
    let forks = discover::load_mapping(cli.mapping.as_deref(), root.as_deref())?;
    let mut rows = discover::collect(root.as_deref(), &forks);

    let mut note = None;
    // Kept around for the TUI's `r` (refresh) key.
    let mut token = None;
    if cli.offline {
        note = Some("offline: PR status not fetched".to_owned());
    } else if let Some(found) = github::token() {
        let prs: Vec<model::PrRef> = rows.iter().filter_map(|row| row.pr.clone()).collect();
        if prs.is_empty() {
            note = Some("no patch references an upstream PR yet".to_owned());
        } else {
            eprintln!("fetching status for {} PRs...", prs.len());
            // A failed fetch (network, rate limit, bad token) degrades to the
            // same no-status listing the tokenless path shows, instead of
            // aborting before anything renders.
            match github::fetch(&prs, &found) {
                Ok(statuses) => {
                    for row in &mut rows {
                        row.status = row
                            .pr
                            .as_ref()
                            .and_then(|pr| statuses.get(&pr.url))
                            .cloned();
                    }
                }
                Err(err) => {
                    note = Some(format!(
                        "PR status fetch failed ({err}); showing patches without live status"
                    ));
                }
            }
        }
        token = Some(found);
    } else {
        note = Some(
            "no GitHub token (set GITHUB_TOKEN/GH_TOKEN or `gh auth login`); \
             showing patches without live PR status"
                .to_owned(),
        );
    }

    if cli.plain || !std::io::stdout().is_terminal() {
        let mut stdout = std::io::stdout().lock();
        plain::print(&rows, &mut stdout)?;
        if let Some(note) = note {
            eprintln!("note: {note}");
        }
        return Ok(());
    }
    tui::run(rows, note, token)
}
