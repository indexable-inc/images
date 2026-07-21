//! `upstream-sync [<pkg> [<patch>]] [--open] [--dry-run] [--check-stale]
//! [--fail-on-red-ci]`: drive the de-fork UPSTREAMING loop. This is the layer above `upstream-pr`
//! (the per-patch branch/am/push/PR mechanism) and `rebase-patches` (the
//! base-bump regenerator): it decides which patches to act on from the
//! hand-written declarative intent, tracks the live state of the PRs we
//! open, spots duplicate upstream PRs, and retires patches that land
//! upstream. `upstream-sync drift [--json|--markdown] [name]` is the
//! read-only companion report (see [`upstream_sync::drift`]).
//!
//! The two-sided design the user set:
//!   - DECLARATIVE INTENT lives in nix (`lib/fork-packages.nix`),
//!     hand-written: each patch's `upstream = attempt|hold|never` + one-line
//!     reason, and a per-repo `upstreamPolicy` (prsWelcome / aiPrsAllowed /
//!     citation / notes). `attempt` is the human gate that authorizes the
//!     outward act; the tool opens a real upstream PR ONLY for a patch
//!     explicitly marked `attempt`.
//!   - LIVE STATE is GENERATED, never hand-written: see
//!     [`upstream_sync::status`].
//!
//! The loop, per `attempt` patch of each selected fork:
//!   1. If we already track a PR: refresh its state via `gh pr view` (open /
//!      draft / merged / closed) AND its upstream CI verdict
//!      (`statusCheckRollup` -> ci = passing | failing | pending | none,
//!      plus the failing check names), logging transitions. A red upstream
//!      PR is reported loudly in the plan summary; with `--fail-on-red-ci`
//!      the run exits nonzero so a cron
//!      (.github/workflows/upstream-pr-watch.yml) surfaces it as a failed
//!      workflow instead of letting it sit unnoticed (nushell/nushell#18549
//!      sat red for days). If merged, mark `retired = true` and record it:
//!      the NEXT base bump's `rebase-patches` run should drop the patch (it
//!      becomes an empty cherry against the new base), and this tool wires a
//!      retirement note into the plan so a human/agent verifies the drop.
//!   2. Else search the upstream repo for a DUPLICATE/related PR by the
//!      patch's title keywords. If found, RECORD it and SKIP loudly (a human
//!      or agent can comment on the existing PR instead of opening a
//!      competing one).
//!   3. Else, if `--open` was passed, open the PR by delegating to
//!      `upstream-pr --open` (its DAG-closure/am/preflight/push/PR
//!      mechanism, one owner; PRs open ready for review, with the body
//!      rendered into the target repo's PR template when it ships one).
//!      For forks opted into the closure build gates
//!      (`closureGates = true`; RFC 0010 A3), the patch's gate derivation
//!      (`forkClosureGates.<system>.<fork>.<patch>`) is built FIRST and a
//!      red gate aborts that patch's PR-opening: `upstream-pr` ships the
//!      patch as its dag.json closure against the bare base, so a
//!      non-building closure means the upstream PR would be broken. Only the
//!      `--open` path pays this build.
//!
//! Opening a PR is the outward act, DOUBLY gated: the patch must be marked
//! `attempt` in nix (intent gate) AND `--open` must be passed (invocation
//! gate). Without `--open`, the safe default, the tool still
//! refreshes/searches/retires and writes the status file, and reports which
//! patches WOULD open. `--dry-run` additionally suppresses the status write.
//!
//! Repos where PRs are unwelcome (`prsWelcome = false`) or AI PRs are banned
//! (`aiPrsAllowed = "false"`) are skipped at the repo level: the tool
//! refuses to open any PR there regardless of a per-patch `attempt`, so a
//! banned repo cannot leak a PR.

use std::path::{Path, PathBuf};

use anstream::println;
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use color_eyre::eyre::{Result, WrapErr, bail};
use lazy_regex::regex;
use upstream_sync::mapping::{self, Fork, Slug};
use upstream_sync::style::{CYAN, GREEN, RED, YELLOW, paint};
use upstream_sync::{cmd, dag, drift, gh, patch, status};

#[derive(Parser)]
#[command(name = "upstream-sync")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    sync: SyncArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Read-only drift report: pinned base vs upstream default branch
    Drift(DriftArgs),
}

#[derive(Args)]
struct DriftArgs {
    /// one fork package (nix | btop | ...); all if omitted
    name: Option<String>,
    /// machine-readable JSON to stdout, nothing else
    #[arg(long)]
    json: bool,
    /// GitHub-flavored markdown table (step summaries, PR bodies)
    #[arg(long)]
    markdown: bool,
    /// fork-package JSON to drive (default: the baked-in list)
    #[arg(long)]
    mapping: Option<PathBuf>,
}

#[derive(Args)]
struct SyncArgs {
    /// one fork package (nix | btop | ...); all if omitted
    pkg: Option<String>,
    /// restrict to one patch file (name/prefix/substring)
    patch: Option<String>,
    /// OPEN real upstream PRs for attempt patches (the outward act; default: refresh + plan only)
    #[arg(long)]
    open: bool,
    /// plan only: refresh + search but write NO status files (pure validation)
    #[arg(long)]
    dry_run: bool,
    /// warn if a fork has attempt patches but no status file, or a stale lastChecked
    #[arg(long)]
    check_stale: bool,
    /// exit nonzero when any tracked open/draft upstream PR has failing CI (for the watch cron)
    #[arg(long)]
    fail_on_red_ci: bool,
    /// fork-package JSON to drive (default: the baked-in list)
    #[arg(long)]
    mapping: Option<PathBuf>,
}

/// One row of the run's decision summary.
struct PlanEntry {
    fork: String,
    patch: String,
    action: String,
    detail: String,
}

/// One tracked open/draft PR whose upstream CI is failing, collected for the
/// end-of-run red report (and the `--fail-on-red-ci` verdict).
struct RedPr {
    fork: String,
    patch: String,
    url: String,
    failing: Vec<String>,
}

/// The run's accumulated decisions: the plan summary plus the tracked PRs
/// with red upstream CI.
#[derive(Default)]
struct Report {
    plan: Vec<PlanEntry>,
    red: Vec<RedPr>,
}

/// Per-fork context threaded through the patch loop.
struct ForkCtx<'a> {
    fork: &'a Fork,
    slug: &'a Slug,
    patch_dir: &'a Path,
    repo_blocked: bool,
    repo_block_reason: &'a str,
    mapping: Option<&'a Path>,
    open: bool,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Drift(args)) => drift::run(
            args.mapping.as_deref(),
            args.name.as_deref(),
            args.json,
            args.markdown,
        ),
        None => run_sync(&cli.sync),
    }
}

fn run_sync(args: &SyncArgs) -> Result<()> {
    let mapping_path = mapping::path(args.mapping.as_deref())?;
    let forks = mapping::select(
        mapping::load(&mapping_path)?,
        args.pkg.as_deref(),
        "upstream-sync",
    )?;
    let mut report = Report::default();
    for fork in &forks {
        process_fork(fork, args, &mut report)?;
    }
    print_plan(&report.plan, args);
    report_red(&report.red, args.fail_on_red_ci)
}

/// Repo-level gates: a non-github host has no gh path; PRs unwelcome or AI
/// banned means we never open here. We still LOAD + report status, but skip
/// any outward act.
struct RepoGate {
    blocked: bool,
    reason: String,
}

fn repo_gate(fork: &Fork, slug: &Slug, gh_ok: bool) -> RepoGate {
    let policy = fork.policy();
    let blocked = !policy.prs_welcome || policy.ai_prs_allowed == "false" || !gh_ok;
    let reason = if !gh_ok {
        format!(
            "upstream is not GitHub ({}/{}); gh path N/A",
            slug.owner, slug.repo
        )
    } else if !policy.prs_welcome {
        "policy: prsWelcome = false".to_owned()
    } else if policy.ai_prs_allowed == "false" {
        format!("policy: aiPrsAllowed = false; see {}", policy.citation)
    } else {
        String::new()
    };
    RepoGate { blocked, reason }
}

fn process_fork(fork: &Fork, args: &SyncArgs, report: &mut Report) -> Result<()> {
    let slug = Slug::parse(&fork.url)?;
    let patch_dir = fork.patch_dir_abs();
    let policy = fork.policy();
    let gh_ok = mapping::is_github(&fork.url);
    let RepoGate {
        blocked: repo_blocked,
        reason: repo_block_reason,
    } = repo_gate(fork, &slug, gh_ok);

    println!(
        "{}",
        paint(
            CYAN,
            &format!("== {} [{}/{}] ==", fork.name, slug.owner, slug.repo)
        )
    );
    if repo_blocked {
        println!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-sync: {}: repo-level block: {repo_block_reason}. No PR will be opened here.",
                    fork.name
                )
            )
        );
    }
    if policy.ai_prs_allowed == "unknown" && gh_ok && policy.prs_welcome {
        println!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-sync: {}: AI-PR policy is UNSTATED upstream; proceeding for attempt patches with AI attribution in the PR body. Citation: {}",
                    fork.name, policy.citation
                )
            )
        );
    }

    // Pre-run committed state, captured before this run touches it, so the
    // `--check-stale` verdict reflects what was actually committed rather
    // than the file this run is about to write.
    let status_path = status::path(fork);
    let pre_existed = status_path.exists();
    let mut doc = status::Doc::load(&status_path)?;
    let pre_last_checked = doc.last_checked.clone();
    doc.last_checked = Some(status::utc_stamp());

    // The patch set to walk: dag.json node order (canonical), filtered by
    // the optional `patch` arg.
    let dag_file = patch_dir.join("dag.json");
    if !dag_file.exists() {
        println!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-sync: {}: no dag.json; run `nix run .#rebase-patches -- dag {}`. Skipping.",
                    fork.name, fork.name
                )
            )
        );
        return Ok(());
    }
    let all_patches = dag::Doc::load(&dag_file)?.patch_names();
    let selected: Vec<String> = match args.patch.as_deref() {
        None => all_patches,
        Some(wanted) => all_patches
            .into_iter()
            .filter(|p| p == wanted || p.starts_with(wanted) || p.contains(wanted))
            .collect(),
    };
    if selected.is_empty()
        && let Some(wanted) = args.patch.as_deref()
    {
        println!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-sync: {}: no patch matching '{wanted}'.",
                    fork.name
                )
            )
        );
    }

    let ctx = ForkCtx {
        fork,
        slug: &slug,
        patch_dir: &patch_dir,
        repo_blocked,
        repo_block_reason: &repo_block_reason,
        mapping: args.mapping.as_deref(),
        open: args.open,
    };
    for pf in &selected {
        handle_patch(&ctx, pf, &mut doc, report)?;
    }

    doc.save(&fork.name, &status_path, args.dry_run)?;
    if args.check_stale {
        check_stale(fork, pre_existed, pre_last_checked.as_deref())?;
    }
    Ok(())
}

fn handle_patch(ctx: &ForkCtx, pf: &str, doc: &mut status::Doc, report: &mut Report) -> Result<()> {
    let stance = ctx.fork.stance(pf);

    // Ensure a status entry exists (mirror intent for legibility).
    let entry = doc
        .patches
        .entry(pf.to_owned())
        .or_insert_with(|| status::Entry {
            upstream: stance.clone(),
            pr: None,
            retired: false,
            duplicates: Vec::new(),
        });
    entry.upstream.clone_from(&stance);

    if stance != "attempt" {
        // Not authorized for the outward act; record intent, no action.
        report.plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: pf.to_owned(),
            action: "skip".to_owned(),
            detail: ctx.fork.reason(pf),
        });
        return Ok(());
    }

    // attempt patch. Repo-level block still wins (defense in depth).
    if ctx.repo_blocked {
        report.plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: pf.to_owned(),
            action: "blocked".to_owned(),
            detail: ctx.repo_block_reason.to_owned(),
        });
        return Ok(());
    }

    // 1. Already tracking a PR? Refresh its state.
    if let Some(tracked) = doc.patches.get(pf).and_then(|e| e.pr.clone()) {
        return refresh_tracked(ctx, pf, &tracked, doc, report);
    }

    // 2. No tracked PR: search for a duplicate before opening.
    let subject = patch::subject(&ctx.patch_dir.join(pf))?;
    let dupes = gh::find_duplicates(ctx.slug, &subject)?;
    if let Some(first) = dupes.first() {
        let first_url = first.url.clone();
        let count = dupes.len();
        if let Some(entry) = doc.patches.get_mut(pf) {
            entry.duplicates = dupes;
        }
        doc.append_log(&format!(
            "{pf}: found {count} possible duplicate upstream PRs; NOT opening. First: {first_url}"
        ));
        report.plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: pf.to_owned(),
            action: "duplicate".to_owned(),
            detail: first_url,
        });
        return Ok(());
    }

    // 3. No PR, no duplicate: open one ONLY when --open was passed. Without
    // it (the safe default) this is a would-open plan entry: the status file
    // still records the pending attempt, but no PR is created.
    if !ctx.open {
        report.plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: pf.to_owned(),
            action: "would-open".to_owned(),
            detail: format!(
                "run with --open to create: upstream-pr --open {} {pf}",
                ctx.fork.name
            ),
        });
        return Ok(());
    }
    open_one(ctx, pf, doc, report)
}

fn refresh_tracked(
    ctx: &ForkCtx,
    pf: &str,
    tracked: &status::Pr,
    doc: &mut status::Doc,
    report: &mut Report,
) -> Result<()> {
    let Some(fresh) = gh::refresh_pr(ctx.slug, tracked.number)? else {
        doc.append_log(&format!(
            "{pf}: tracked PR #{} no longer readable, deleted or renamed; leaving last-known state",
            tracked.number
        ));
        report.plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: pf.to_owned(),
            action: "stale-pr".to_owned(),
            detail: format!("PR #{} unreadable", tracked.number),
        });
        return Ok(());
    };

    // Log a state transition when it changed.
    if fresh.state != tracked.state {
        doc.append_log(&format!(
            "{pf}: PR #{} {} -> {} ({})",
            fresh.number, tracked.state, fresh.state, fresh.url
        ));
    }

    // Log CI transitions while the PR is live (a merged/closed PR's checks
    // no longer matter). Pre-CI-tracking entries deserialize with
    // `ci = "none"`, so the first refresh logs the real verdict as a visible
    // transition. A failing verdict joins the end-of-run red report.
    let live = fresh.state == "open" || fresh.state == "draft";
    if live {
        if fresh.ci != tracked.ci {
            let ci_detail = if fresh.ci == "failing" {
                format!(" [{}]", fresh.failing_checks.join(", "))
            } else {
                String::new()
            };
            doc.append_log(&format!(
                "{pf}: PR #{} CI {} -> {}{ci_detail}",
                fresh.number, tracked.ci, fresh.ci
            ));
        }
        if fresh.ci == "failing" {
            report.red.push(RedPr {
                fork: ctx.fork.name.clone(),
                patch: pf.to_owned(),
                url: fresh.url.clone(),
                failing: fresh.failing_checks.clone(),
            });
        }
    }

    // Merged upstream -> retire. The next base bump's rebase-patches run
    // should drop the patch (it cherries empty against the new base); we
    // wire that verification into the plan for a human/agent to confirm.
    let newly_merged = fresh.state == "merged" && doc.patches.get(pf).is_some_and(|e| !e.retired);
    if let Some(entry) = doc.patches.get_mut(pf) {
        entry.pr = Some(fresh.clone());
        if newly_merged {
            entry.retired = true;
        }
    }
    if newly_merged {
        doc.append_log(&format!("{pf}: merged upstream in PR #{}; marked retired. Verify the next base bump drops it as an empty cherry.", fresh.number));
    }

    let action = if fresh.state == "merged" {
        "retired".to_owned()
    } else {
        format!("tracked:{}", fresh.state)
    };
    let detail = if live {
        format!("{} [ci: {}]", fresh.url, fresh.ci)
    } else {
        fresh.url
    };
    report.plan.push(PlanEntry {
        fork: ctx.fork.name.clone(),
        patch: pf.to_owned(),
        action,
        detail,
    });
    Ok(())
}

/// Closure-gate preflight (RFC 0010 A3, #2098): `upstream-pr` ships this
/// patch as its dag.json ancestor closure against the bare base, so for
/// forks opted in via `closureGates = true` prove that closure BUILDS before
/// the outward act. The gate attr is the current repo flake's (a downstream
/// --mapping repo gates against its own flake).
fn closure_gate_passes(
    ctx: &ForkCtx,
    pf: &str,
    doc: &mut status::Doc,
    report: &mut Report,
) -> Result<bool> {
    let system = cmd::run("nix", &["config", "show", "system"])?;
    let gate = format!(".#forkClosureGates.{system}.{}.\"{pf}\"", ctx.fork.name);
    println!(
        "{}",
        paint(
            CYAN,
            &format!(
                "upstream-sync: {}: building closure gate {gate} before opening (heavy full-package build; cache hit when unchanged)",
                ctx.fork.name
            )
        )
    );
    let res = cmd::complete("nix", &["build", "--no-link", &gate])?;
    if res.ok() {
        return Ok(true);
    }
    println!("{}", res.stderr);
    println!(
        "{}",
        paint(
            RED,
            &format!(
                "upstream-sync: {}: closure gate FAILED for {pf}: its dag.json closure does not build standalone, so the upstream PR would ship broken. Fix the series; NOT opening.",
                ctx.fork.name
            )
        )
    );
    doc.append_log(&format!(
        "{pf}: closure gate build FAILED; PR-opening aborted"
    ));
    report.plan.push(PlanEntry {
        fork: ctx.fork.name.clone(),
        patch: pf.to_owned(),
        action: "gate-failed".to_owned(),
        detail: gate,
    });
    Ok(false)
}

/// The outward act, only for attempt patches on a non-blocked repo, only
/// when --open was passed. `upstream-pr` owns the
/// branch/am/preflight/push/PR mechanism (ready for review by default);
/// --mapping is threaded so a downstream repo's list is used.
fn open_one(ctx: &ForkCtx, pf: &str, doc: &mut status::Doc, report: &mut Report) -> Result<()> {
    if ctx.fork.closure_gates && !closure_gate_passes(ctx, pf, doc, report)? {
        return Ok(());
    }

    println!(
        "{}",
        paint(
            GREEN,
            &format!(
                "upstream-sync: {}: opening upstream PR for {pf} via upstream-pr --open",
                ctx.fork.name
            )
        )
    );
    let mut argv: Vec<String> = vec!["--open".to_owned()];
    if let Some(m) = ctx.mapping {
        argv.extend(["--mapping".to_owned(), m.display().to_string()]);
    }
    argv.extend([ctx.fork.name.clone(), pf.to_owned()]);
    let opened = cmd::complete("upstream-pr", &argv)?;
    println!("{}", opened.stdout);
    if !opened.ok() {
        println!(
            "{}",
            paint(
                RED,
                &format!(
                    "upstream-sync: {}: upstream-pr failed for {pf}:",
                    ctx.fork.name
                )
            )
        );
        println!("{}", opened.stderr);
        doc.append_log(&format!(
            "{pf}: upstream-pr --open FAILED; see output above"
        ));
        report.plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: pf.to_owned(),
            action: "open-failed".to_owned(),
            detail: "upstream-pr error".to_owned(),
        });
        return Ok(());
    }

    // Parse the created PR URL from upstream-pr's output (gh prints it on
    // `pr create`). Best-effort: if we cannot parse it, still log the act.
    let pr_url = opened
        .stdout
        .lines()
        .rfind(|l| l.contains("github.com") && l.contains("/pull/"))
        .map(str::to_owned);
    if let Some(url) = &pr_url {
        let number = regex!(r"/pull/([0-9]+)")
            .captures(url)
            .and_then(|c| c.get(1))
            .map_or(0, |m| m.as_str().parse().unwrap_or(0));
        // upstream-pr opens ready for review (its preflight + template
        // rendering are the pre-submit bar); CI has not reported yet, so the
        // verdict starts pending and the next refresh settles it.
        let fresh = status::Pr {
            url: url.trim().to_owned(),
            number,
            state: "open".to_owned(),
            ci: "pending".to_owned(),
            failing_checks: Vec::new(),
            checked_at: status::utc_stamp(),
        };
        let opened_url = fresh.url.clone();
        if let Some(entry) = doc.patches.get_mut(pf) {
            entry.pr = Some(fresh);
        }
        doc.append_log(&format!("{pf}: opened PR {opened_url}"));
    } else {
        doc.append_log(&format!(
            "{pf}: upstream-pr --open succeeded but PR URL was not parseable from output"
        ));
    }
    report.plan.push(PlanEntry {
        fork: ctx.fork.name.clone(),
        patch: pf.to_owned(),
        action: "opened".to_owned(),
        detail: pr_url.unwrap_or_else(|| "unknown".to_owned()),
    });
    Ok(())
}

/// Staleness verdicts judge the PRE-run committed state (captured at load),
/// so they are meaningful in every mode, including right after this run
/// wrote a fresh file.
fn check_stale(fork: &Fork, pre_existed: bool, pre_last_checked: Option<&str>) -> Result<()> {
    let attempts = fork
        .patches
        .values()
        .filter(|m| m.upstream.as_deref().unwrap_or("hold") == "attempt")
        .count();
    if attempts > 0 && !pre_existed {
        println!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-sync: {}: STALE: has {attempts} attempt patches but no committed upstream-status.json; run a non-dry-run sync and commit it.",
                    fork.name
                )
            )
        );
        return Ok(());
    }
    let Some(prev) = pre_last_checked else {
        return Ok(());
    };
    let prev_dt = DateTime::parse_from_rfc3339(prev)
        .wrap_err_with(|| format!("unparseable lastChecked {prev}"))?;
    // 14 days: tracked-PR state and the duplicate landscape move on the
    // scale of weeks; older than that and the committed state is a stale
    // basis for the next upstreaming decision.
    let age = Utc::now().signed_duration_since(prev_dt);
    if age > chrono::Duration::days(14) {
        println!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-sync: {}: STALE: committed upstream-status.json was last checked {prev}, {} days ago; re-run and commit.",
                    fork.name,
                    age.num_days()
                )
            )
        );
    }
    Ok(())
}

/// Grouped by action, one patch per line with its full detail (no table
/// truncation), so the output pastes straight into a PR body / plan review.
fn print_plan(plan: &[PlanEntry], args: &SyncArgs) {
    println!();
    println!(
        "{}",
        paint(
            CYAN,
            &format!("== upstream-sync plan: {} patch decisions ==", plan.len())
        )
    );
    if plan.is_empty() {
        println!("  (no patches selected)");
    }
    let mut groups: Vec<(&str, Vec<&PlanEntry>)> = Vec::new();
    for entry in plan {
        match groups
            .iter_mut()
            .find(|(action, _)| *action == entry.action)
        {
            Some((_, rows)) => rows.push(entry),
            None => groups.push((&entry.action, vec![entry])),
        }
    }
    for (action, rows) in &groups {
        println!("{}", paint(CYAN, &format!("[{action}] {}", rows.len())));
        for r in rows {
            println!("  {} / {}", r.fork, r.patch);
            println!("      {}", r.detail);
        }
    }

    let ready: Vec<&PlanEntry> = plan
        .iter()
        .filter(|r| r.action == "would-open" || r.action == "opened")
        .collect();
    if !ready.is_empty() {
        println!();
        println!(
            "{}",
            paint(
                GREEN,
                &format!(
                    "attempt-ready patches ({}): these are the outward-PR candidates.",
                    ready.len()
                )
            )
        );
        for r in &ready {
            println!("  - {} / {}", r.fork, r.patch);
        }
        if !args.open {
            println!(
                "{}",
                paint(
                    YELLOW,
                    "Re-run with --open to create these PRs; opening is the outward act."
                )
            );
        }
    }
    if args.dry_run {
        println!();
        println!(
            "{}",
            paint(
                YELLOW,
                "--dry-run: no status files written. Drop --dry-run to persist the refreshed status; add --open to create PRs."
            )
        );
    }
}

/// Red upstream CI is always reported loudly; with `--fail-on-red-ci` it
/// also fails the run, so the scheduled watch workflow
/// (.github/workflows/upstream-pr-watch.yml) turns red instead of letting a
/// failing upstream PR sit unnoticed under our name.
fn report_red(red: &[RedPr], fail_on_red_ci: bool) -> Result<()> {
    if red.is_empty() {
        return Ok(());
    }
    println!();
    println!(
        "{}",
        paint(
            RED,
            &format!(
                "upstream CI is RED on {} tracked PR(s) we opened:",
                red.len()
            )
        )
    );
    for r in red {
        println!("  - {} / {}: {}", r.fork, r.patch, r.url);
        println!("      failing: {}", r.failing.join(", "));
    }
    if fail_on_red_ci {
        bail!(
            "upstream-sync: tracked upstream PRs have failing CI (listed above); fix the patch series and force-push via upstream-pr, or close the PR. A red PR under our name is a negative signal upstream."
        );
    }
    Ok(())
}
