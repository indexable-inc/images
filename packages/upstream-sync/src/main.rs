//! `upstream-sync [<pkg> [<patch>]] [--open] [--dry-run] [--check-stale]`:
//! drive the de-fork UPSTREAMING loop. This is the layer above `upstream-pr`
//! (the per-patch branch/push/PR mechanism): it decides which patches to act
//! on from the hand-written declarative intent, tracks the live state of the
//! PRs we open, spots duplicate upstream PRs, and retires patches that land
//! upstream. `upstream-sync drift [--json|--markdown] [name]` is the
//! read-only companion report (see [`upstream_sync::drift`]).
//!
//! The patch series lives in each fork repo's commit DAG (the jj megamerge
//! layout, see [`upstream_sync::series`]): every patch is a commit whose
//! parents are its true dependencies, and its identity is its SUBJECT line,
//! which survives jj rebases. The loop opens a scratch commits-only clone
//! per fork and walks that series; there are no in-repo patch files.
//!
//! The two-sided design the user set:
//!   - DECLARATIVE INTENT lives in nix (`lib/fork-packages.nix`),
//!     hand-written: each patch's `upstream = attempt|hold|never` + one-line
//!     reason (keyed by commit subject), and a per-repo `upstreamPolicy`
//!     (prsWelcome / aiPrsAllowed / citation / notes). `attempt` is the
//!     human gate that authorizes the outward act; the tool opens a real
//!     upstream PR ONLY for a patch explicitly marked `attempt`.
//!   - LIVE STATE is GENERATED, never hand-written: see
//!     [`upstream_sync::status`].
//!
//! The loop, per `attempt` patch of each selected fork:
//!   1. If we already track a PR: refresh its state via `gh pr view`. If
//!      merged, mark `retired = true` and record it: the NEXT fork-repo
//!      rebase onto upstream should drop the patch (it becomes an empty
//!      commit against the new base), and this tool wires a retirement note
//!      into the plan so a human/agent verifies the drop.
//!   2. Else search the upstream repo for a DUPLICATE/related PR by the
//!      patch's subject keywords. If found, RECORD it and SKIP loudly (a
//!      human or agent can comment on the existing PR instead of opening a
//!      competing one).
//!   3. Else, if `--open` was passed, open the PR by delegating to
//!      `upstream-pr --open` (branch push + draft PR, one owner). The
//!      patch's commit ancestry IS its dependency closure, so what ships is
//!      well-formed by construction; no separate build gate runs here.
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

use std::path::PathBuf;

use anstream::println;
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand};
use color_eyre::eyre::{Result, WrapErr, eyre};
use lazy_regex::regex;
use upstream_sync::mapping::{self, Fork, Slug};
use upstream_sync::style::{CYAN, GREEN, RED, YELLOW, paint};
use upstream_sync::{cmd, drift, gh, notify, series, status};

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
    /// Regenerate the committed org roster that drives PR @-mentions
    Members(MembersArgs),
    /// Bring every tracked PR's @-mention block in line with the roster
    Notify(NotifyArgs),
    /// Check lib/fork-packages.nix for unknown stances and unexplained gates
    Validate(ValidateArgs),
}

#[derive(Args)]
struct MembersArgs {
    /// GitHub org to read
    #[arg(long, default_value = "indexable-inc")]
    org: String,
    /// write the roster file; default is to print what would change
    #[arg(long)]
    write: bool,
    /// roster path to write (default: the baked-in one)
    #[arg(long)]
    members: Option<PathBuf>,
}

#[derive(Args)]
struct NotifyArgs {
    /// one fork package (nix | btop | ...); all if omitted
    pkg: Option<String>,
    /// EDIT the PR bodies; default is to report what would change
    #[arg(long)]
    apply: bool,
    /// fork-package JSON to drive (default: the baked-in list)
    #[arg(long)]
    mapping: Option<PathBuf>,
    /// roster JSON to drive (default: the baked-in one)
    #[arg(long)]
    members: Option<PathBuf>,
}

#[derive(Args)]
struct ValidateArgs {
    /// fork-package JSON to check (default: the baked-in list)
    #[arg(long)]
    mapping: Option<PathBuf>,
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
    /// restrict to one patch (commit subject / prefix / substring)
    patch: Option<String>,
    #[command(flatten)]
    act: ActArgs,
    /// plan only: refresh + search but write NO status files (pure validation)
    #[arg(long)]
    dry_run: bool,
    /// warn if a fork has attempt patches but no status file, or a stale lastChecked
    #[arg(long)]
    check_stale: bool,
    /// fork-package JSON to drive (default: the baked-in list)
    #[arg(long)]
    mapping: Option<PathBuf>,
}

// The two flags that authorize the outward act, together because they are
// read together: `--open` is the invocation gate a human passes, `--auto`
// narrows the same act to repos that opted in. Every decision to open a PR
// consults both, so they travel as one value rather than two booleans
// threaded separately.
//
// A plain comment, not a doc comment: clap renders a flattened struct's doc
// comment as a group heading in `--help`, so five lines of rationale would
// land in front of the user every time they asked for usage.
#[derive(Args, Clone, Copy)]
struct ActArgs {
    /// OPEN real upstream PRs for attempt patches (the outward act; default: refresh + plan only)
    #[arg(long)]
    open: bool,
    /// unattended mode: act only on forks that opted in via upstreamPolicy.autoContribute
    #[arg(long)]
    auto: bool,
}

/// One row of the run's decision summary.
struct PlanEntry {
    fork: String,
    patch: String,
    action: String,
    detail: String,
}

/// Per-fork context threaded through the patch loop.
struct ForkCtx<'a> {
    fork: &'a Fork,
    slug: &'a Slug,
    repo_blocked: bool,
    repo_block_reason: &'a str,
    mapping: Option<&'a std::path::Path>,
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
        Some(Command::Members(args)) => run_members(&args),
        Some(Command::Notify(args)) => run_notify(&args),
        Some(Command::Validate(args)) => run_validate(&args),
        None => run_sync(&cli.sync),
    }
}

/// Regenerate the committed roster. Writing is change-gated on the rendered
/// bytes so a re-run with no membership change leaves the file, and the
/// commit history, untouched.
fn run_members(args: &MembersArgs) -> Result<()> {
    let path = notify::path(args.members.as_deref())?;
    let previous = notify::Roster::load(&path).ok();
    // Carry the old stamp forward when nothing else moved, so an unchanged
    // roster produces byte-identical output instead of a timestamp-only diff.
    let stamp = previous
        .as_ref()
        .map_or_else(status::utc_stamp, |p| p.generated_at.clone());
    let mut fresh = notify::Roster::fetch(&args.org, stamp)?;

    let changed = previous
        .as_ref()
        .is_none_or(|p| p.members.len() != fresh.members.len() || p.humans() != fresh.humans());
    if changed {
        fresh.generated_at = status::utc_stamp();
    }

    let humans = fresh.humans();
    println!(
        "upstream-sync: members: {} in {}, {} human: {}",
        fresh.members.len(),
        fresh.org,
        humans.len(),
        humans.join(", ")
    );
    if !changed {
        println!(
            "{}",
            paint(
                GREEN,
                "upstream-sync: members: roster unchanged; nothing written."
            )
        );
        return Ok(());
    }
    if !args.write {
        println!(
            "{}",
            paint(
                YELLOW,
                "upstream-sync: members: roster CHANGED; re-run with --write to commit it."
            )
        );
        return Ok(());
    }
    std::fs::write(&path, fresh.to_bytes()?)
        .wrap_err_with(|| format!("cannot write {}", path.display()))?;
    println!(
        "{}",
        paint(
            GREEN,
            &format!("upstream-sync: members: wrote {}", path.display())
        )
    );
    Ok(())
}

/// Reconcile the mention block on every PR this repo tracks.
///
/// Reads the committed status files rather than the forks' series: a PR we
/// opened is tracked there whatever the patch's stance has since become, so
/// a patch marked `rejected` still gets its block kept current while the
/// conversation on it continues.
fn run_notify(args: &NotifyArgs) -> Result<()> {
    let roster = notify::Roster::load(&notify::path(args.members.as_deref())?)?;
    let Some(block) = roster.block() else {
        println!(
            "{}",
            paint(
                YELLOW,
                "upstream-sync: notify: roster has no human members; nothing to mention."
            )
        );
        return Ok(());
    };
    let forks = mapping::select(
        mapping::load(&mapping::path(args.mapping.as_deref())?)?,
        args.pkg.as_deref(),
        "upstream-sync notify",
    )?;

    let mut seen = 0_u32;
    let mut wrote = 0_u32;
    for fork in &forks {
        if !mapping::is_github(&fork.upstream_url) {
            continue;
        }
        let path = status::path(fork);
        if !path.exists() {
            continue;
        }
        let slug = Slug::parse(&fork.upstream_url)?;
        let mut doc = status::Doc::load(&path)?;
        let tracked: Vec<(String, u64)> = doc
            .patches
            .iter()
            .filter_map(|(subject, e)| e.pr.as_ref().map(|pr| (subject.clone(), pr.number)))
            .collect();
        let mut touched = false;
        for (subject, number) in tracked {
            seen += 1;
            let outcome = notify::reconcile(&slug, number, &block, args.apply)?;
            let line = format!(
                "upstream-sync: notify: {}#{number} ({subject}): {}",
                fork.name,
                outcome.word()
            );
            if outcome == notify::Outcome::Unchanged {
                println!("{line}");
                continue;
            }
            if !args.apply {
                println!(
                    "{}",
                    paint(YELLOW, &format!("{line} (would; pass --apply)"))
                );
                continue;
            }
            println!("{}", paint(GREEN, &line));
            doc.append_log(&format!(
                "{subject}: notify block {} on PR #{number}",
                outcome.word()
            ));
            touched = true;
            wrote += 1;
        }
        if touched {
            doc.save(&fork.name, &path, false)?;
        }
    }
    println!(
        "{}",
        paint(
            CYAN,
            &format!("upstream-sync: notify: {seen} tracked PR(s), {wrote} edited.")
        )
    );
    Ok(())
}

/// Check the registry and say so, so the gate has a command a check can run.
fn run_validate(args: &ValidateArgs) -> Result<()> {
    let forks = mapping::load(&mapping::path(args.mapping.as_deref())?)?;
    mapping::validate(&forks)?;
    println!(
        "{}",
        paint(
            GREEN,
            &format!(
                "upstream-sync: validate: {} fork(s) OK: every stance known, every autoContribute explained.",
                forks.len()
            )
        )
    );
    Ok(())
}

fn run_sync(args: &SyncArgs) -> Result<()> {
    let mapping_path = mapping::path(args.mapping.as_deref())?;
    let all = mapping::load(&mapping_path)?;
    // A bad registry changes what gets contributed silently, so it fails the
    // run before any forge call rather than after some of them.
    mapping::validate(&all)?;
    let forks = mapping::select(all, args.pkg.as_deref(), "upstream-sync")?;
    let mut plan: Vec<PlanEntry> = Vec::new();
    // Per-fork isolation. A fork whose series cannot be read is a data
    // problem in that fork, and aborting the loop there hides every fork
    // after it: mesa's series carries two commits with the same subject,
    // which halted the whole run at fork 8 of 13, so nix and the four small
    // forks were never looked at and the failure read as "the tool is
    // broken" rather than "mesa needs a retitle". Collect and report them
    // all, then fail, so one bad fork costs one row and not the run.
    let mut failures: Vec<(String, String)> = Vec::new();
    for fork in &forks {
        if let Err(err) = process_fork(fork, args, &mut plan) {
            println!(
                "{}",
                paint(
                    RED,
                    &format!("upstream-sync: {}: FAILED: {err:#}", fork.name)
                )
            );
            failures.push((fork.name.clone(), format!("{err:#}")));
        }
    }
    print_plan(&plan, args);
    if !failures.is_empty() {
        return Err(eyre!(
            "upstream-sync: {} of {} fork(s) failed:\n  - {}",
            failures.len(),
            forks.len(),
            failures
                .iter()
                .map(|(name, err)| format!("{name}: {err}"))
                .collect::<Vec<_>>()
                .join("\n  - ")
        ));
    }
    Ok(())
}

/// Repo-level gates: a non-github host has no gh path; PRs unwelcome or AI
/// banned means we never open here. We still LOAD + report status, but skip
/// any outward act.
struct RepoGate {
    blocked: bool,
    reason: String,
}

fn repo_gate(fork: &Fork, slug: &Slug, gh_ok: bool, auto: bool) -> RepoGate {
    let policy = fork.policy();
    // Unattended mode adds one gate to the two that already existed. The
    // others ask whether a PR is acceptable at all; this one asks whether it
    // is acceptable with nobody watching, which a repo can answer
    // differently: ghostty welcomes AI-assisted PRs and still auto-closes an
    // unvouched contributor's.
    let auto_blocked = auto && !policy.auto_contribute.enabled;
    let blocked = !policy.prs_welcome || policy.ai_prs_allowed == "false" || !gh_ok || auto_blocked;
    let reason = if !gh_ok {
        format!(
            "upstream is not GitHub ({}/{}); gh path N/A",
            slug.owner, slug.repo
        )
    } else if !policy.prs_welcome {
        "policy: prsWelcome = false".to_owned()
    } else if policy.ai_prs_allowed == "false" {
        format!("policy: aiPrsAllowed = false; see {}", policy.citation)
    } else if auto_blocked {
        format!(
            "policy: autoContribute.enabled = false. {}",
            policy.auto_contribute.reason
        )
    } else {
        String::new()
    };
    RepoGate { blocked, reason }
}

/// Every intent key must name a real series commit: a key orphaned by a
/// rebase that retitled or dropped its commit is dead intent, and dead
/// `attempt` intent silently loses the authorization it encodes.
fn ensure_no_orphaned_intent(fork: &Fork, all_subjects: &[String]) -> Result<()> {
    let orphaned: Vec<&String> = fork
        .patches
        .keys()
        .filter(|key| !all_subjects.iter().any(|s| s == *key))
        .collect();
    if orphaned.is_empty() {
        return Ok(());
    }
    Err(eyre!(
        "upstream-sync: {}: intent keys in lib/fork-packages.nix match no commit subject on {}@{}: {:?}. Series subjects: {:?}",
        fork.name,
        fork.fork_repo,
        fork.bookmark,
        orphaned,
        all_subjects
    ))
}

fn process_fork(fork: &Fork, args: &SyncArgs, plan: &mut Vec<PlanEntry>) -> Result<()> {
    let slug = Slug::parse(&fork.upstream_url)?;
    let policy = fork.policy();
    let gh_ok = mapping::is_github(&fork.upstream_url);
    let RepoGate {
        blocked: repo_blocked,
        reason: repo_block_reason,
    } = repo_gate(fork, &slug, gh_ok, args.act.auto);

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

    // The patch set to walk: the fork repo's commit series (scratch
    // commits-only clone), filtered by the optional `patch` arg.
    println!(
        "upstream-sync: {}: reading series from {}@{}...",
        fork.name, fork.fork_repo, fork.bookmark
    );
    let repo = series::Repo::open(fork)?;
    let all_subjects = repo.subjects();
    ensure_no_orphaned_intent(fork, &all_subjects)?;

    let selected: Vec<String> = match args.patch.as_deref() {
        None => all_subjects,
        Some(wanted) => all_subjects
            .into_iter()
            .filter(|s| s == wanted || s.starts_with(wanted) || s.contains(wanted))
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
        repo_blocked,
        repo_block_reason: &repo_block_reason,
        mapping: args.mapping.as_deref(),
        open: args.act.open,
    };
    for subject in &selected {
        handle_patch(&ctx, subject, &mut doc, plan)?;
    }

    doc.save(&fork.name, &status_path, args.dry_run)?;
    if args.check_stale {
        check_stale(fork, pre_existed, pre_last_checked.as_deref())?;
    }
    Ok(())
}

fn handle_patch(
    ctx: &ForkCtx,
    subject: &str,
    doc: &mut status::Doc,
    plan: &mut Vec<PlanEntry>,
) -> Result<()> {
    let stance = ctx.fork.stance(subject);

    // Ensure a status entry exists (mirror intent for legibility).
    let entry = doc
        .patches
        .entry(subject.to_owned())
        .or_insert_with(|| status::Entry {
            upstream: stance.clone(),
            pr: None,
            retired: false,
            duplicates: Vec::new(),
        });
    entry.upstream.clone_from(&stance);

    if stance != "attempt" {
        // Not authorized for the outward act; record intent, no action.
        // `rejected` gets its own row because it is the one stance that
        // records an answer already received, and a reader scanning the plan
        // should be able to see how many patches upstream has turned down
        // without reading every reason.
        plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: subject.to_owned(),
            action: if stance == "rejected" {
                "rejected".to_owned()
            } else {
                "skip".to_owned()
            },
            detail: ctx.fork.reason(subject),
        });
        return Ok(());
    }

    // attempt patch. Repo-level block still wins (defense in depth).
    if ctx.repo_blocked {
        plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: subject.to_owned(),
            action: "blocked".to_owned(),
            detail: ctx.repo_block_reason.to_owned(),
        });
        return Ok(());
    }

    // 1. Already tracking a PR? Refresh its state.
    if let Some(tracked) = doc.patches.get(subject).and_then(|e| e.pr.clone()) {
        return refresh_tracked(ctx, subject, &tracked, doc, plan);
    }

    // 2. No tracked PR in our state, but there may be one on the forge: a
    // `upstream-pr --open` run by hand pushes the branch and opens the PR
    // without going through this loop, so the status file never learned
    // about it. Ask about our own head branch first; the duplicate search
    // below cannot tell our PR from a competing one and would skip the
    // patch as a duplicate of itself.
    let branch = format!("upstream/{}", series::slug(subject));
    if let Some(ours) = gh::find_ours(ctx.slug, ctx.fork.fork_owner(), &branch)? {
        doc.append_log(&format!(
            "{subject}: adopted existing PR #{} from our branch {branch}",
            ours.number
        ));
        if let Some(entry) = doc.patches.get_mut(subject) {
            // A previous run may have recorded this very PR as a competing
            // one. It is ours; leaving it in the duplicates list would read
            // as someone else having proposed the same change.
            entry.duplicates.retain(|d| d.number != ours.number);
        }
        return refresh_tracked(ctx, subject, &ours, doc, plan);
    }

    // 3. Nothing of ours: search for a duplicate before opening.
    let dupes = gh::find_duplicates(ctx.slug, subject)?;
    if let Some(first) = dupes.first() {
        let first_url = first.url.clone();
        let count = dupes.len();
        if let Some(entry) = doc.patches.get_mut(subject) {
            entry.duplicates = dupes;
        }
        doc.append_log(&format!(
            "{subject}: found {count} possible duplicate upstream PRs; NOT opening. First: {first_url}"
        ));
        plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: subject.to_owned(),
            action: "duplicate".to_owned(),
            detail: first_url,
        });
        return Ok(());
    }

    // 4. No PR, no duplicate: open one ONLY when --open was passed. Without
    // it (the safe default) this is a would-open plan entry: the status file
    // still records the pending attempt, but no PR is created.
    if !ctx.open {
        plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: subject.to_owned(),
            action: "would-open".to_owned(),
            detail: format!(
                "run with --open to create: upstream-pr --open {} '{subject}'",
                ctx.fork.name
            ),
        });
        return Ok(());
    }
    open_one(ctx, subject, doc, plan)
}

fn refresh_tracked(
    ctx: &ForkCtx,
    subject: &str,
    tracked: &status::Pr,
    doc: &mut status::Doc,
    plan: &mut Vec<PlanEntry>,
) -> Result<()> {
    let Some(fresh) = gh::refresh_pr(ctx.slug, tracked.number)? else {
        doc.append_log(&format!(
            "{subject}: tracked PR #{} no longer readable, deleted or renamed; leaving last-known state",
            tracked.number
        ));
        plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: subject.to_owned(),
            action: "stale-pr".to_owned(),
            detail: format!("PR #{} unreadable", tracked.number),
        });
        return Ok(());
    };

    // Log a state transition when it changed.
    if fresh.state != tracked.state {
        doc.append_log(&format!(
            "{subject}: PR #{} {} -> {} ({})",
            fresh.number, tracked.state, fresh.state, fresh.url
        ));
    }

    // Merged upstream -> retire. The next fork-repo rebase onto upstream
    // should drop the patch (it becomes empty against the new base); we
    // wire that verification into the plan for a human/agent to confirm.
    let newly_merged =
        fresh.state == "merged" && doc.patches.get(subject).is_some_and(|e| !e.retired);
    if let Some(entry) = doc.patches.get_mut(subject) {
        entry.pr = Some(fresh.clone());
        if newly_merged {
            entry.retired = true;
        }
    }
    if newly_merged {
        doc.append_log(&format!("{subject}: merged upstream in PR #{}; marked retired. Verify the next rebase drops it as an empty commit.", fresh.number));
    }

    // Closed without merging is upstream's answer, and leaving the stance at
    // `attempt` records the opposite of what happened. Nothing reopens the
    // PR (a tracked PR is never re-created), so this is a report, not a
    // block: mark it `rejected` and the plan stops claiming we are still
    // trying.
    if fresh.state == "closed" && ctx.fork.stance(subject) == "attempt" {
        println!(
            "{}",
            paint(
                RED,
                &format!(
                    "upstream-sync: {}: PR #{} for '{subject}' was CLOSED unmerged. Set upstream = \"rejected\" with the reason in lib/fork-packages.nix so the registry stops claiming we are still attempting it.",
                    ctx.fork.name, fresh.number
                )
            )
        );
    }

    // A PR we opened and left red is the failure mode this tool could not
    // see before: nushell#18549 sat with two failing checks and no reply for
    // weeks. Say it every run, in the colour that means act.
    if let Some(checks) = fresh
        .checks
        .as_ref()
        .filter(|c| c.red() && fresh.state != "merged")
    {
        println!(
            "{}",
            paint(
                RED,
                &format!(
                    "upstream-sync: {}: PR #{} for '{subject}' is RED upstream ({}). {}",
                    ctx.fork.name,
                    fresh.number,
                    checks.summary(),
                    fresh.url
                )
            )
        );
    }

    let action = if fresh.state == "merged" {
        "retired".to_owned()
    } else if fresh.checks.as_ref().is_some_and(status::Checks::red) {
        format!("tracked:{}:RED", fresh.state)
    } else {
        format!("tracked:{}", fresh.state)
    };
    plan.push(PlanEntry {
        fork: ctx.fork.name.clone(),
        patch: subject.to_owned(),
        action,
        detail: fresh.url,
    });
    Ok(())
}

/// The outward act, only for attempt patches on a non-blocked repo, only
/// when --open was passed. `upstream-pr` owns the branch-push/draft-PR
/// mechanism; --mapping is threaded so a downstream repo's list is used.
fn open_one(
    ctx: &ForkCtx,
    subject: &str,
    doc: &mut status::Doc,
    plan: &mut Vec<PlanEntry>,
) -> Result<()> {
    println!(
        "{}",
        paint(
            GREEN,
            &format!(
                "upstream-sync: {}: opening upstream PR for '{subject}' via upstream-pr --open",
                ctx.fork.name
            )
        )
    );
    let mut argv: Vec<String> = vec!["--open".to_owned()];
    if let Some(m) = ctx.mapping {
        argv.extend(["--mapping".to_owned(), m.display().to_string()]);
    }
    argv.extend([ctx.fork.name.clone(), subject.to_owned()]);
    let opened = cmd::complete("upstream-pr", &argv)?;
    println!("{}", opened.stdout);
    if !opened.ok() {
        println!(
            "{}",
            paint(
                RED,
                &format!(
                    "upstream-sync: {}: upstream-pr failed for '{subject}':",
                    ctx.fork.name
                )
            )
        );
        println!("{}", opened.stderr);
        doc.append_log(&format!(
            "{subject}: upstream-pr --open FAILED; see output above"
        ));
        plan.push(PlanEntry {
            fork: ctx.fork.name.clone(),
            patch: subject.to_owned(),
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
        let fresh = status::Pr {
            url: url.trim().to_owned(),
            number,
            state: "draft".to_owned(),
            // A PR seconds old has no checks yet; the next run's refresh
            // fills them in. Recording an empty tally here would read as
            // "no CI on this repo".
            checks: None,
            checked_at: status::utc_stamp(),
        };
        let opened_url = fresh.url.clone();
        if let Some(entry) = doc.patches.get_mut(subject) {
            entry.pr = Some(fresh);
        }
        doc.append_log(&format!("{subject}: opened draft PR {opened_url}"));
    } else {
        doc.append_log(&format!(
            "{subject}: upstream-pr --open succeeded but PR URL was not parseable from output"
        ));
    }
    plan.push(PlanEntry {
        fork: ctx.fork.name.clone(),
        patch: subject.to_owned(),
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
                    "upstream-sync: {}: STALE: has {attempts} attempt patches but no committed status file; run a non-dry-run sync and commit it.",
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
                    "upstream-sync: {}: STALE: committed status file was last checked {prev}, {} days ago; re-run and commit.",
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
        if !args.act.open {
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
