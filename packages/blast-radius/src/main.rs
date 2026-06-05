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
use std::time::Instant;

use clap::Parser;
use color_eyre::eyre::{Result, bail};

use causes::{Caps, root_causes};
use report::{Report, categories};

/// Phase labels are kebab-case and stable: they appear in CI logs and in
/// `report.json.phaseTimings`, so renaming them breaks downstream readers.
fn record_phase(phases: &mut BTreeMap<String, f64>, label: &'static str, secs: f64) {
    eprintln!("blast-radius: {label}: {secs:.2}s");
    phases.insert(label.to_owned(), secs);
}

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

/// The two catalog evaluations a report diffs. Named (rather than a bare
/// tuple) to satisfy the workspace's `clippy::anonymous_tuple_return_type`.
struct Evals {
    base: nix::EvalResult,
    head: nix::EvalResult,
}

/// Evaluate the catalog at base and head in parallel, timing each thread's
/// own work so the recorded seconds reflect that worker's compute and not the
/// parent scope's wall clock. The gap between `eval-base + eval-head` and
/// `total` is the parallelism dividend.
///
/// The eval cache stays off (see [`nix::eval_checks`]): two concurrent
/// `nix-eval-jobs` contend on the per-commit eval-cache `SQLite`.
fn concurrent_evals(
    repo: &str,
    base: &str,
    head: &str,
    catalog: &str,
    phases: &mut BTreeMap<String, f64>,
) -> Result<Evals> {
    let (base, head) = std::thread::scope(|scope| {
        let head_h = scope.spawn(|| {
            let t = Instant::now();
            (nix::eval_checks(repo, head, catalog), t.elapsed().as_secs_f64())
        });
        let t = Instant::now();
        let base = (nix::eval_checks(repo, base, catalog), t.elapsed().as_secs_f64());
        let head = head_h
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (base, head)
    });
    record_phase(phases, "eval-base", base.1);
    record_phase(phases, "eval-head", head.1);
    Ok(Evals {
        base: base.0?,
        head: head.0?,
    })
}

/// Walk the changed-check frontier into a ranked cause list. Returns an
/// empty list when nothing changed: the caller only renders causes that fan
/// out from one of the rebuilt drvs.
fn compute_causes(
    base: &[nix::Check],
    head: &[nix::Check],
    changed: &[String],
    phases: &mut BTreeMap<String, f64>,
) -> Result<Vec<causes::Cause>> {
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    // Full `.drv` paths feed `nix derivation show`; the resulting graphs are
    // keyed by basename, so attribute a check to its head drv's basename.
    let head_paths: Vec<String> = changed
        .iter()
        .filter_map(|attr| nix::drv_for(head, attr))
        .collect();
    let base_paths: Vec<String> = changed
        .iter()
        .filter_map(|attr| nix::drv_for(base, attr))
        .collect();
    let t = Instant::now();
    let head_graph = nix::derivation_graph(&head_paths)?;
    record_phase(phases, "derivation-show-head", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let base_graph = nix::derivation_graph(&base_paths)?;
    record_phase(phases, "derivation-show-base", t.elapsed().as_secs_f64());
    let changed_basenames: BTreeMap<String, String> = changed
        .iter()
        .filter_map(|attr| {
            nix::drv_for(head, attr).map(|path| (attr.clone(), nix::basename(&path).to_owned()))
        })
        .collect();
    let t = Instant::now();
    let causes = root_causes(&base_graph, &head_graph, &changed_basenames, CAPS);
    record_phase(phases, "root-causes", t.elapsed().as_secs_f64());
    Ok(causes)
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let wall = Instant::now();
    let mut phases: BTreeMap<String, f64> = BTreeMap::new();
    let revs = git::resolve(cli.base.as_deref(), cli.head.as_deref())?;

    // Pick one catalog output for both revisions so their attr names line up
    // (see nix::catalog_attr): `ciChecks` when both revs expose it, else flat
    // `checks`. Resolved before the eval scope so base and head never mix keying.
    let t = Instant::now();
    let catalog = nix::catalog_attr(&revs.repo, &revs.base, &revs.head);
    record_phase(&mut phases, "catalog-probe", t.elapsed().as_secs_f64());

    let Evals { base, head } =
        concurrent_evals(&revs.repo, &revs.base, &revs.head, catalog, &mut phases)?;
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

    let causes = compute_causes(&base, &head, &changed, &mut phases)?;

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

    record_phase(&mut phases, "total", wall.elapsed().as_secs_f64());

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
        phase_timings: phases,
    };

    if cli.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print!("{}", report.to_markdown());
    }
    Ok(())
}
