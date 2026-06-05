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

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use clap::Parser;
use color_eyre::eyre::{Result, bail};

use causes::{Caps, root_causes};
use report::{Report, categories};

/// Graph budget for the rendered flowchart: only the highest fan-out causes, and
/// a few checks each, are drawn. The changed-checks list stays complete.
const CAPS: Caps = Caps {
    max_causes: 6,
    max_checks_per_cause: 5,
};

/// The two catalog evaluations a report diffs. A named struct rather than a bare
/// `(EvalResult, EvalResult)` so the concurrent-eval scope below has a
/// self-documenting return (and satisfies `clippy::anonymous_tuple_return_type`).
struct Evals {
    base: nix::EvalResult,
    head: nix::EvalResult,
}

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

/// Fail closed if any check fails to evaluate at head that did not already fail
/// at base. A failure new at head is a regression this change introduced (a
/// check it broke, or a broken check it added), so the run aborts rather than
/// render a successful-looking report that hides it. Failures present at base
/// too are a pre-existing catalog issue (a `.#checks` set is not guaranteed
/// eval-clean: ix carries eval-assertion checks that throw when their invariant
/// is violated and are not in its required gate); those have no derivation, so
/// they are excluded from the diff and only reported.
fn guard_eval_failures(base: &nix::EvalResult, head: &nix::EvalResult) -> Result<()> {
    // Tolerate by attribute NAME, not by error text: some eval errors embed
    // rev-varying store paths (e.g. an `*-no-nix-dependencies` check names the
    // offending `.drv`), so comparing payloads would read the same pre-existing
    // failure as new and false-bail on every run. An attr unevaluable at both
    // base and head has no derivation at either, so it is out of rebuild scope
    // regardless of WHY it fails.
    let base_attrs: BTreeSet<&str> = base.failures.iter().map(|f| f.attr.as_str()).collect();
    let new_failures: Vec<&nix::EvalFailure> = head
        .failures
        .iter()
        .filter(|f| !base_attrs.contains(f.attr.as_str()))
        .collect();
    if !new_failures.is_empty() {
        // Surface the full error text: per-attr eval failures exit nix-eval-jobs
        // 0, so this bail is the only place the cause reaches the CI log.
        let detail = new_failures
            .iter()
            .map(|f| format!("  {}: {}", f.attr, f.error))
            .collect::<Vec<_>>()
            .join("\n");
        bail!(
            "{} check(s) newly fail to evaluate at head (this change broke them):\n{detail}",
            new_failures.len()
        );
    }
    if !base.failures.is_empty() {
        let names = base
            .failures
            .iter()
            .map(|f| f.attr.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "blast-radius: {} check(s) fail to evaluate at base and head; excluded from the diff: {names}",
            base.failures.len()
        );
    }
    Ok(())
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let revs = git::resolve(cli.base.as_deref(), cli.head.as_deref())?;

    // Pick one catalog output for both revisions so their attr names line up
    // (see nix::catalog_attr): `ciChecks` when both revs expose it, else flat
    // `checks`. Resolved before the eval scope so base and head never mix keying.
    let catalog = nix::catalog_attr(&revs.repo, &revs.base, &revs.head);

    // base and head evals are independent, so run them concurrently. Each is a
    // full `#{catalog}.x86_64-linux` evaluation (~11 min on ix's ~4300 checks),
    // mostly blocked on the per-unit cargo IFD builds, so overlapping them
    // roughly halves the wall clock versus back-to-back. The eval cache stays
    // off (see eval_checks): with it on, two concurrent nix-eval-jobs contend on
    // the per-commit eval-cache SQLite and fail with "database is busy".
    let Evals { base, head } = std::thread::scope(|scope| -> Result<Evals> {
        let head_eval = scope.spawn(|| nix::eval_checks(&revs.repo, &revs.head, catalog));
        let base = nix::eval_checks(&revs.repo, &revs.base, catalog)?;
        let head = match head_eval.join() {
            Ok(result) => result?,
            Err(_) => bail!("head check evaluation thread panicked"),
        };
        Ok(Evals { base, head })
    })?;
    guard_eval_failures(&base, &head)?;
    let base = base.checks;
    let head = head.checks;

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

    // Best-effort: a present-but-unreadable or corrupt timings file (a partial
    // artifact download, an empty upload) must not fail the report and break the
    // PR comment. The workflow already decides *whether* to pass --timings; if it
    // does and the file is bad, warn and continue with no annotations rather than
    // aborting.
    let timings = cli.timings.as_deref().map_or_else(BTreeMap::new, |path| {
        timings::load(path).unwrap_or_else(|err| {
            eprintln!(
                "blast-radius: ignoring unreadable timings file {}: {err:?}",
                path.display()
            );
            BTreeMap::new()
        })
    });

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
