//! `blast-radius`: report how many `.#checks.x86_64-linux` derivations a PR
//! would rebuild, and which changed inputs caused each rebuild.
//!
//! The untrusted half of `.github/workflows/blast-radius.yml` runs this with
//! `--json` to produce `report.json`; the trusted half validates that shape and
//! renders the sticky PR comment from it. Run without `--json` locally to see
//! the same report as Markdown.

mod causes;
mod git;
mod nix;
mod report;
mod timings;

use std::collections::BTreeMap;
use std::path::PathBuf;

use clap::Parser;
use color_eyre::eyre::Result;

use causes::{Caps, root_causes};
use report::{Report, categories};

/// Graph budget for the rendered flowchart: only the highest fan-out causes, and
/// a few checks each, are drawn. The changed-checks list stays complete.
const CAPS: Caps = Caps {
    max_causes: 6,
    max_checks_per_cause: 5,
};

#[derive(Parser)]
#[command(
    about = "Report how many .#checks.x86_64-linux derivations a PR would rebuild, and why"
)]
struct Cli {
    /// Base ref to diff against (default: origin/main).
    base: Option<String>,
    /// Head ref to report on (default: HEAD).
    head: Option<String>,
    /// Emit the machine-readable report.json instead of Markdown.
    #[arg(long)]
    json: bool,
    /// nix-fast-build `check-results.json` from a prior successful Check run
    /// (typically the base branch). Used to annotate the rebuilt-checks list
    /// with per-attr wall-clock seconds. Missing attrs are omitted, not zeroed.
    #[arg(long, value_name = "PATH")]
    timings: Option<PathBuf>,
}

fn short(rev: &str) -> String {
    rev.chars().take(7).collect()
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let revs = git::resolve(cli.base.as_deref(), cli.head.as_deref())?;

    let base = nix::eval_checks(&revs.repo, &revs.base)?;
    let head = nix::eval_checks(&revs.repo, &revs.head)?;

    let base_map: BTreeMap<&str, &str> = base
        .iter()
        .map(|check| (check.attr.as_str(), check.drv_path.as_str()))
        .collect();
    let head_map: BTreeMap<&str, &str> = head
        .iter()
        .map(|check| (check.attr.as_str(), check.drv_path.as_str()))
        .collect();

    let mut changed: Vec<String> = head
        .iter()
        .filter(|check| {
            base_map
                .get(check.attr.as_str())
                .is_some_and(|base_drv| *base_drv != check.drv_path)
        })
        .map(|check| check.attr.clone())
        .collect();
    changed.sort();
    let mut added: Vec<String> = head
        .iter()
        .filter(|check| !base_map.contains_key(check.attr.as_str()))
        .map(|check| check.attr.clone())
        .collect();
    added.sort();
    let mut removed: Vec<String> = base
        .iter()
        .filter(|check| !head_map.contains_key(check.attr.as_str()))
        .map(|check| check.attr.clone())
        .collect();
    removed.sort();
    let total = head.len();

    let causes = if changed.is_empty() {
        Vec::new()
    } else {
        // Full `.drv` paths feed `nix derivation show`; the resulting graphs are
        // keyed by basename, so attribute a check to its head drv's basename.
        let head_paths: Vec<String> = changed
            .iter()
            .filter_map(|attr| nix::drv_for(&head, attr))
            .collect();
        let base_paths: Vec<String> = changed
            .iter()
            .filter_map(|attr| nix::drv_for(&base, attr))
            .collect();
        let head_graph = nix::derivation_graph(&head_paths)?;
        let base_graph = nix::derivation_graph(&base_paths)?;
        let changed_basenames: BTreeMap<String, String> = changed
            .iter()
            .filter_map(|attr| {
                nix::drv_for(&head, attr).map(|path| (attr.clone(), nix::basename(&path).to_owned()))
            })
            .collect();
        root_causes(&base_graph, &head_graph, &changed_basenames, CAPS)
    };

    let timings = match cli.timings.as_deref() {
        Some(path) => timings::load(path)?,
        None => BTreeMap::new(),
    };

    let report = Report {
        base: short(&revs.base),
        head: short(&revs.head),
        total,
        categories: categories(&changed, &added),
        causes: causes.into_iter().map(Into::into).collect(),
        changed,
        added,
        removed,
        timings,
    };

    if cli.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print!("{}", report.to_markdown());
    }
    Ok(())
}
