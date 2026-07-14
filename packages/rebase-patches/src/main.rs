//! `rebase-patches`: regenerate a de-forked package's patch series via a real
//! git rebase when its upstream base moves, plus its dependency DAG, and the
//! `dag-check` invariant driver run by the `patch-dag-<name>` flake checks.
//! Run from the repo root; see src/rebase.rs and src/check.rs for the
//! mechanics.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod check;
mod dag;
mod fork;
mod git;
mod rebase;

#[derive(Parser)]
#[command(
    name = "rebase-patches",
    about = "Regenerate a de-forked package's patch series via a real git rebase when its upstream base moves, and its dependency DAG"
)]
struct Cli {
    /// One fork package (codex | btop | clippy | mesa); all changed if omitted.
    name: Option<String>,
    /// Fork-package JSON to drive (default: index's baked-in list). A
    /// downstream repo points this one tool at its own fork list, run from its
    /// repo root so patchDir and flake.lock resolve there.
    #[arg(long, global = true)]
    mapping: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Continue a stopped rebase in the scratch repo, then publish the patches
    /// and DAG exactly as a conflict-free run would.
    Resume { name: String, scratch: PathBuf },
    /// Regenerate dag.json for one or all fork packages against the
    /// currently-pinned base (working-tree flake.lock), without a rebase.
    Dag { name: Option<String> },
    /// Invariant driver for the `patch-dag-<name>` flake check: verify the
    /// committed dag.json against a src tree, patch dir, pinned base, and
    /// upstreaming-intent JSON. Exits non-zero on any violation.
    DagCheck {
        src_dir: PathBuf,
        patch_dir: PathBuf,
        expected_base: String,
        #[arg(default_value = "{}")]
        intent_json: String,
    },
    /// Plumbing for packages/upstream-pr (folds into its Rust rewrite, #3249):
    /// print a patch's ancestor closure from a dag.json, one name per line in
    /// series (NNNN) order, excluding the patch itself.
    #[command(hide = true)]
    DagClosure { dag_json: PathBuf, patch: String },
    /// Plumbing for packages/upstream-pr (folds into its Rust rewrite, #3249):
    /// make an existing git repo's behavior config-independent (rerere off,
    /// no signing, no autocrlf) so a developer's global git settings cannot
    /// perturb apply-tests.
    #[command(hide = true)]
    NeutralizeConfig { repo: PathBuf },
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let mapping = cli.mapping.as_deref();
    match cli.command {
        Some(Command::Resume { name, scratch }) => rebase::resume(&name, &scratch, mapping)?,
        Some(Command::Dag { name }) => rebase::dag_all(name.as_deref(), mapping)?,
        Some(Command::DagCheck { src_dir, patch_dir, expected_base, intent_json }) => {
            if !check::run(&src_dir, &patch_dir, &expected_base, &intent_json)? {
                return Ok(ExitCode::FAILURE);
            }
        }
        Some(Command::DagClosure { dag_json, patch }) => {
            let doc: dag::Document = serde_json::from_str(
                &fs::read_to_string(&dag_json)
                    .with_context(|| format!("read {}", dag_json.display()))?,
            )
            .with_context(|| format!("parse {}", dag_json.display()))?;
            let deps_of: BTreeMap<String, Vec<String>> = doc
                .nodes
                .iter()
                .map(|n| (n.patch.clone(), n.deps.clone()))
                .collect();
            let series_pos: BTreeMap<&str, usize> = doc
                .nodes
                .iter()
                .enumerate()
                .map(|(pos, n)| (n.patch.as_str(), pos))
                .collect();
            let mut names = dag::closure(&deps_of, &patch)?;
            names.sort_by_key(|n| series_pos[n.as_str()]);
            for name in names {
                println!("{name}");
            }
        }
        Some(Command::NeutralizeConfig { repo }) => dag::neutralize_config(&repo)?,
        None => rebase::run(cli.name.as_deref(), mapping)?,
    }
    Ok(ExitCode::SUCCESS)
}
