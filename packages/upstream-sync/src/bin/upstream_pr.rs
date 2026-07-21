//! `upstream-pr <pkg> <patch> [--open] [--draft] [--dry-run]`: contribute
//! ONE of our fork patches upstream without carrying the rest of the series.
//!
//! We keep a de-forked patch series (packages/<pkg>/patches, see
//! `lib/util/patched-src.nix`) pinned at an OLDER upstream base. To send a
//! single patch upstream, we cannot just push our whole branch: it drags in
//! every other patch and is based on a stale rev. So this tool:
//!
//!   1. Reads the patch's ancestor closure from dag.json (the derived
//!      dependency graph). A truly independent patch contributes just
//!      itself; a patch with real deps drags its closure, and we warn
//!      listing the extra patches so the author knows the upstream PR is not
//!      single-commit.
//!   2. Fetches the upstream repo's DEFAULT branch tip (not our pinned
//!      base), so the contribution targets current upstream.
//!   3. `git am --3way` the closure onto that tip. The 3-way merge absorbs
//!      mechanical drift between our old base and the upstream tip; a real
//!      collision fails loudly (this is exactly where old-base-vs-tip drift
//!      surfaces, and a human must rebase the patch).
//!   4. Runs the fork's `preflight` commands (lib/fork-packages.nix) in the
//!      patched scratch checkout: the target repo's own cheap pre-submit
//!      gates (fmt-level, mirroring the first steps of its CI). A red
//!      preflight aborts the contribution loudly BEFORE anything is pushed;
//!      an upstream PR that fails `cargo fmt` in its first CI step reads as
//!      low-effort to maintainers (nushell/nushell#18549).
//!   5. Pushes the branch to an indexable-inc fork of the upstream repo
//!      (created with `gh repo fork --clone=false` if absent). Pushing to
//!      OUR fork is fine; it is not the outward act.
//!   6. Prints the ready-to-open compare URL. With `--open`, additionally
//!      opens the PR upstream READY FOR REVIEW (pass `--draft` to open a
//!      draft instead). Ready is the default because the preflight and
//!      template rendering above are exactly the pre-submit bar; a PR parked
//!      as a draft signals not-ready and sits unreviewed. Default is
//!      prepare-only: opening the upstream PR is the outward act and stays
//!      behind an explicit `--open` a human invokes.
//!
//! The PR's title and body come FROM THE PATCH ITSELF: subject = title,
//! commit message body = PR body (one fact, one home; the fork mapping
//! deliberately has no duplicate description field), plus AI attribution and
//! a link back to the patch file of record. When the target repo ships a PR
//! template (.github/pull_request_template.md and the standard fallback
//! locations, read from the scratch checkout), the body is RENDERED INTO the
//! template's `## ` sections instead: Description <- the commit body, a
//! release-notes section <- the patch's `releaseNotes` from the mapping, an
//! additional-notes section <- `prExtra` + the attribution block (see
//! [`upstream_sync::template`]). A template section this tool cannot fill
//! refuses loudly rather than opening a noncompliant PR; a repo with no
//! template keeps the plain composition (body + `prExtra` + attribution). An
//! optional `patches.<patch>.prExtra` in the mapping carries
//! upstream-specific PR content (issue refs, checklists) that does not
//! belong in a commit message. A body-less commit is refused; the
//! `patch-dag-<name>` check enforces the same for every attempt-marked patch
//! so the failure happens in CI, not mid-contribution.
//!
//! `--dry-run` runs the whole flow (closure, fetch, am, branch, preflight,
//! body composition including template rendering) but skips the push and PR,
//! printing what it WOULD push and open. Used to validate content without
//! touching any remote.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anstream::println;
use clap::Parser;
use color_eyre::eyre::{Result, WrapErr, bail, eyre};
use lazy_regex::regex;
use upstream_sync::mapping::{self, Fork, Slug};
use upstream_sync::style::{CYAN, GREEN, YELLOW, paint};
use upstream_sync::{cmd, dag, patch, template};

/// The GitHub org whose forks host the contribution branches.
const ORG: &str = "indexable-inc";

#[derive(Parser)]
#[command(name = "upstream-pr")]
struct Cli {
    /// fork package name (codex | btop | clippy)
    pkg: String,
    /// patch file name (or its NNNN prefix / unique substring)
    patch: String,
    /// also open the PR upstream (outward act; default: prepare only)
    #[arg(long)]
    open: bool,
    /// with --open: open the PR as a draft (default: ready for review; the
    /// preflight + template rendering are the pre-submit bar, and a draft
    /// parked on a maintainer's queue signals not-ready and sits unreviewed)
    #[arg(long)]
    draft: bool,
    /// run the whole flow but skip push + PR (validate content)
    #[arg(long)]
    dry_run: bool,
    /// fork-package JSON to drive (default: the baked-in list)
    #[arg(long)]
    mapping: Option<PathBuf>,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    let mapping_path = mapping::path(cli.mapping.as_deref())?;
    let mut forks = mapping::select(mapping::load(&mapping_path)?, Some(&cli.pkg), "upstream-pr")?;
    let fork = forks.swap_remove(0);
    let patch_dir = fork.patch_dir_abs();
    let dag_file = patch_dir.join("dag.json");
    if !dag_file.exists() {
        bail!(
            "upstream-pr: {}: missing dag.json in {}; run `nix run .#rebase-patches -- dag`",
            cli.pkg,
            fork.patch_dir
        );
    }
    let doc = dag::Doc::load(&dag_file)?;
    let Closure { target, ordered } = resolve_closure(&cli.pkg, &cli.patch, &doc)?;

    let slug = Slug::parse(&fork.url)?;
    let branch = format!("upstream-pr/{}/{}", cli.pkg, patch::slug(&target));

    // Scratch repo: fetch the upstream DEFAULT branch tip and `git am` the
    // closure onto it with 3-way. The dir survives failures and --dry-run
    // for inspection; only the success path removes it.
    let scratch = tempfile::Builder::new()
        .prefix(&format!("upstream-pr-{}.", cli.pkg))
        .tempdir()
        .wrap_err("cannot create scratch dir")?
        .keep();
    let PreparedBranch { head_ref, tip } =
        prepare_branch(&scratch, &fork, &slug, &branch, &cli.pkg)?;
    apply_closure(&scratch, &patch_dir, &ordered, &tip, &cli.pkg)?;

    let n_commits = cmd::run_in(
        &scratch,
        "git",
        &["rev-list", "--count", &format!("{tip}..HEAD")],
    )?;
    let tip_short: String = tip.chars().take(10).collect();
    println!(
        "{}",
        paint(
            GREEN,
            &format!(
                "upstream-pr: {}: applied {n_commits} commit(s) cleanly onto {}/{}@{head_ref} ({tip_short})",
                cli.pkg, slug.owner, slug.repo
            )
        )
    );

    run_preflight(&scratch, &fork, &cli.pkg)?;

    if cli.dry_run {
        let content = compose_pr_content(&cli.pkg, &fork, &scratch, &target)?;
        return dry_run_report(&scratch, &tip, &branch, &slug.repo, &content);
    }

    // Compose (and, when the target repo ships a template, render) the PR
    // content BEFORE the push: an unfillable template section refuses here,
    // leaving no half-done outward state behind.
    let content = if cli.open {
        Some(compose_pr_content(&cli.pkg, &fork, &scratch, &target)?)
    } else {
        None
    };

    // Ensure an indexable-inc fork of the upstream exists, then push.
    ensure_fork(&slug)?;
    println!("upstream-pr: pushing {branch} to {ORG}/{}...", slug.repo);
    cmd::run_in(
        &scratch,
        "git",
        &[
            "remote",
            "add",
            "fork",
            &format!("https://github.com/{ORG}/{}.git", slug.repo),
        ],
    )?;
    cmd::run_in(&scratch, "git", &["push", "--force", "fork", &branch])?;

    let compare = format!(
        "https://github.com/{}/{}/compare/{head_ref}...{ORG}:{}:{branch}?expand=1",
        slug.owner, slug.repo, slug.repo
    );
    println!(
        "{}",
        paint(
            GREEN,
            &format!(
                "upstream-pr: {}: pushed. Ready-to-open compare URL:",
                cli.pkg
            )
        )
    );
    println!("  {compare}");

    if let Some(content) = content {
        open_pr(&slug, &head_ref, &branch, &content, cli.draft)?;
    } else {
        println!(
            "upstream-pr: prepare-only. Re-run with `--open` to open the PR upstream (add `--draft` for a draft), or open the compare URL by hand."
        );
    }

    fs::remove_dir_all(&scratch)
        .wrap_err_with(|| format!("cannot remove scratch repo {}", scratch.display()))?;
    Ok(())
}

/// A resolved target patch together with its full contribution closure
/// (ancestors plus the target) in series order.
struct Closure {
    target: String,
    ordered: Vec<String>,
}

/// Resolve the requested patch and compute its contribution closure in
/// series (NNNN) order, ancestors first with the target included; split out
/// of [`main`] to keep it within clippy's function-length budget. Warns when
/// the patch drags ancestors so the author knows the upstream PR is not
/// single-commit.
fn resolve_closure(pkg: &str, requested: &str, doc: &dag::Doc) -> Result<Closure> {
    let all_patches = doc.patch_names();
    // Resolve the requested patch to an exact node name (exact, then prefix,
    // then unique substring).
    let target = dag::resolve(requested, &all_patches)?;
    println!(
        "{}",
        paint(CYAN, &format!("upstream-pr: {pkg}: target patch {target}"))
    );

    // Ancestor closure from the DAG, in NNNN order, plus the target last.
    let mut closure = doc.closure(&target);
    let pos: HashMap<&str, usize> = all_patches
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let by_series = |p: &String| pos.get(p.as_str()).copied().unwrap_or(usize::MAX);
    closure.sort_by_key(by_series);
    let mut ordered = closure.clone();
    ordered.push(target.clone());
    ordered.sort_by_key(by_series);
    ordered.dedup();
    if closure.is_empty() {
        println!("upstream-pr: {pkg}: {target} is independent; contributing it alone.");
    } else {
        println!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-pr: {pkg}: {target} is NOT independent; its upstream contribution drags {} ancestor patch(es):",
                    closure.len()
                )
            )
        );
        for c in &closure {
            println!("  - {c}");
        }
        println!(
            "{}",
            paint(
                YELLOW,
                "upstream-pr: consider splitting, or send the closure as one PR."
            )
        );
    }
    Ok(Closure { target, ordered })
}

/// Per-repo preflight (`preflight` in the fork mapping): the target repo's
/// own cheap pre-submit gates (fmt-level checks mirroring the first steps of
/// its CI, never full test suites), run in the patched scratch checkout so
/// the EXACT tree we would push passes them. A red preflight aborts the
/// contribution loudly before anything is pushed: nushell/nushell#18549
/// shipped a `cargo fmt` failure that turned the whole upstream CI matrix
/// red in seconds. Commands run via `bash -ec` with the invoking
/// environment's toolchain; a missing tool fails the same way (loudly),
/// never skips.
fn run_preflight(scratch: &Path, fork: &Fork, pkg: &str) -> Result<()> {
    for command in &fork.preflight {
        println!("upstream-pr: {pkg}: preflight: {command}");
        let res = cmd::complete_in(scratch, "bash", &["-ec", command])?;
        if !res.ok() {
            println!("{}", res.stdout);
            println!("{}", res.stderr);
            bail!(
                "upstream-pr: {pkg}: preflight `{command}` FAILED in the patched checkout; the upstream PR would open with red CI. Fix the patch series first. Scratch repo: {}",
                scratch.display()
            );
        }
        println!(
            "{}",
            paint(
                GREEN,
                &format!("upstream-pr: {pkg}: preflight `{command}` passed")
            )
        );
    }
    Ok(())
}

/// Print what `--dry-run` would have pushed and opened (branch, commits,
/// composed PR content, scratch repo location); split out of [`main`] to
/// keep it within clippy's function-length budget.
fn dry_run_report(
    scratch: &Path,
    tip: &str,
    branch: &str,
    repo: &str,
    content: &PrContent,
) -> Result<()> {
    println!(
        "{}",
        paint(
            GREEN,
            &format!(
                "upstream-pr: --dry-run: would push branch {branch} to {ORG}/{repo} and print a compare URL. Commits:"
            )
        )
    );
    println!(
        "{}",
        cmd::run_in(
            scratch,
            "git",
            &["log", "--oneline", &format!("{tip}..HEAD")]
        )?
    );
    println!(
        "upstream-pr: --dry-run: with --open the PR would be titled \"{}\" with body:",
        content.title
    );
    println!("{}", content.body);
    println!(
        "upstream-pr: scratch repo left for inspection: {}",
        scratch.display()
    );
    Ok(())
}

/// Deterministic scratch-repo config so a developer's global git settings do
/// not perturb the apply. Mirrors `dag neutralize-config` in
/// `packages/rebase-patches/dag-lib.nu`, that logic's owner until the
/// rebase-patches rewrite (#3250) lands.
fn neutralize_config(scratch: &Path) -> Result<()> {
    for (key, value) in [
        ("rerere.enabled", "false"),
        ("rerere.autoupdate", "false"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "false"),
    ] {
        cmd::run_in(scratch, "git", &["config", key, value])?;
    }
    Ok(())
}

/// Init the scratch repo, discover + fetch the upstream default branch, and
/// check out the contribution branch at its tip.
struct PreparedBranch {
    head_ref: String,
    tip: String,
}

fn prepare_branch(
    scratch: &Path,
    fork: &Fork,
    slug: &Slug,
    branch: &str,
    pkg: &str,
) -> Result<PreparedBranch> {
    cmd::run_in(scratch, "git", &["init", "--quiet"])?;
    neutralize_config(scratch)?;
    println!(
        "upstream-pr: fetching {}/{} default branch tip...",
        slug.owner, slug.repo
    );
    cmd::run_in(scratch, "git", &["remote", "add", "upstream", &fork.url])?;

    // Discover the default branch (HEAD) of upstream, then fetch just it.
    let symref = cmd::run_in(
        scratch,
        "git",
        &["ls-remote", "--symref", "upstream", "HEAD"],
    )?;
    let head_ref = symref
        .lines()
        .find(|l| l.starts_with("ref:"))
        .and_then(|l| regex!(r"ref:\s+refs/heads/(\S+)\s+HEAD").captures(l))
        .map(|c| c[1].to_owned())
        .ok_or_else(|| {
            eyre!(
                "upstream-pr: {pkg}: cannot discover the default branch of {}",
                fork.url
            )
        })?;
    println!("upstream-pr: upstream default branch is {head_ref}");

    cmd::run_in(scratch, "git", &["fetch", "--quiet", "upstream", &head_ref])?;
    let tip = cmd::run_in(scratch, "git", &["rev-parse", "FETCH_HEAD"])?;
    cmd::run_in(scratch, "git", &["checkout", "--quiet", "-b", branch, &tip])?;
    Ok(PreparedBranch { head_ref, tip })
}

/// Apply the closure onto the tip with 3-way. On conflict, fail loudly: this
/// is where our old base drifting from the upstream tip shows up.
fn apply_closure(
    scratch: &Path,
    patch_dir: &Path,
    ordered: &[String],
    tip: &str,
    pkg: &str,
) -> Result<()> {
    let mut am_args: Vec<String> = vec!["am".to_owned(), "--3way".to_owned()];
    am_args.extend(
        ordered
            .iter()
            .map(|p| patch_dir.join(p).display().to_string()),
    );
    let am = cmd::complete_in(scratch, "git", &am_args)?;
    if am.ok() {
        return Ok(());
    }

    let unmerged = cmd::run_in(scratch, "git", &["diff", "--name-only", "--diff-filter=U"])?;
    // `git am --3way` can fail with no unmerged entries when a patch adds a
    // file that already exists upstream, or a hunk has no 3-way base. Fall
    // back to git's own message so the failure is legible either way.
    let detail = if unmerged.is_empty() {
        let combined = format!("{}{}", am.stdout, am.stderr);
        let lines: Vec<&str> = combined.lines().collect();
        let tail = &lines[lines.len().saturating_sub(12)..];
        format!("git am output:\n{}", tail.join("\n"))
    } else {
        format!(
            "conflicting files: [{}]",
            unmerged.lines().collect::<Vec<_>>().join(", ")
        )
    };
    cmd::complete_in(scratch, "git", &["am", "--abort"])?;
    bail!(
        "upstream-pr: {pkg}: `git am --3way` of the closure did not apply onto the upstream tip {tip}. The patch needs rebasing against current upstream before it can be contributed (old-base-vs-tip drift). {detail}. Scratch repo: {}",
        scratch.display()
    );
}

/// The https blob URL of the patch file of record in the INVOKING repo (so a
/// downstream mapping links to its own repo), derived from the `origin`
/// remote in either ssh or https form. Returns `None` with a loud note when
/// origin is absent or not a github URL: the PR body then omits the link
/// rather than fabricating one.
fn origin_blob_link(patch_dir_rel: &str, patch: &str) -> Result<Option<String>> {
    let res = cmd::complete("git", &["remote", "get-url", "origin"])?;
    if !res.ok() {
        println!(
            "{}",
            paint(
                YELLOW,
                "upstream-pr: no `origin` remote here; the PR body will omit the patch-of-record link."
            )
        );
        return Ok(None);
    }
    let url = res.stdout.trim();
    let Some(caps) = regex!(r"github\.com[:/]([^/]+)/([^/]+?)(\.git)?$").captures(url) else {
        println!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-pr: origin {url} is not a parseable github URL; the PR body will omit the patch-of-record link."
                )
            )
        );
        return Ok(None);
    };
    Ok(Some(format!(
        "https://github.com/{}/{}/blob/main/{patch_dir_rel}/{patch}",
        &caps[1], &caps[2]
    )))
}

/// The composed PR content: subject = title, body = the commit-message body
/// rendered into the target repo's template when it ships one, plus
/// `prExtra` and the AI attribution.
struct PrContent {
    title: String,
    body: String,
}

/// Compose the PR title and body from the patch's own commit message (one
/// fact, one home: nix carries no duplicate description field), so a
/// body-less commit is refused loudly; the `patch-dag-<name>` check enforces
/// the same for attempt-marked patches before it ever gets here. Follows the
/// target repo's conventions: the body is rendered into its PR template when
/// it ships one (refusing loudly on any section we cannot fill); the plain
/// composition applies when it does not.
fn compose_pr_content(pkg: &str, fork: &Fork, scratch: &Path, target: &str) -> Result<PrContent> {
    let title = cmd::run_in(scratch, "git", &["log", "-1", "--format=%s", "HEAD"])?;
    let commit_body = cmd::run_in(scratch, "git", &["log", "-1", "--format=%b", "HEAD"])?;
    if commit_body.is_empty() {
        bail!(
            "upstream-pr: {pkg}: {target} has no commit-message body; write the why in the commit body (it becomes the upstream PR description)."
        );
    }

    // Optional upstream-specific PR content that does not belong in a commit
    // message: `prExtra` (issue refs, checklists) and `releaseNotes`
    // (user-facing release-note text for templates that require it),
    // declared per patch in the fork mapping.
    let patch_meta = fork.patches.get(target);
    let pr_extra = patch_meta.and_then(|m| m.pr_extra.clone());
    let release_notes = patch_meta.and_then(|m| m.release_notes.clone());
    // Link back to the patch file of record in OUR repo, derived from the
    // invoking repo's origin remote so a downstream mapping links to its own
    // repo. Best-effort but loud: no parseable origin means no link.
    let patch_link = origin_blob_link(&fork.patch_dir, target)?;
    let attribution = [
        "---".to_owned(),
        patch_link.map_or_else(
            || format!("Contributed from a maintained fork patch series (patch {target})."),
            |link| format!("Contributed from a maintained fork patch series; the patch of record is {link}."),
        ),
        "Prepared with AI assistance (Claude); directed and reviewed by a human maintainer.".to_owned(),
    ]
    .join("\n\n");
    // prExtra + attribution together are the "anything else reviewers should
    // know" content: under the template's additional-notes section when the
    // repo has a template, appended after the body otherwise.
    let mut note_parts: Vec<String> = Vec::new();
    note_parts.extend(pr_extra);
    note_parts.push(attribution);
    let notes = note_parts.join("\n\n");

    let body = match template::find(scratch) {
        Some(template_path) => {
            let shown = template_path
                .strip_prefix(scratch)
                .unwrap_or(&template_path);
            println!(
                "upstream-pr: {pkg}: rendering the PR body into the target repo's template ({})",
                shown.display()
            );
            let raw = fs::read_to_string(&template_path)
                .wrap_err_with(|| format!("cannot read {}", template_path.display()))?;
            template::render(
                &raw,
                pkg,
                target,
                &template::Sections {
                    description: commit_body,
                    release_notes,
                    notes,
                },
            )?
        }
        None => [commit_body, notes].join("\n\n"),
    };
    Ok(PrContent { title, body })
}

/// The outward act, gated behind --open: open the PR upstream, READY FOR
/// REVIEW unless `--draft` was passed. Ready is the default because the
/// preflight and template rendering ARE the pre-submit bar this tool
/// enforces before it gets here, and a draft parked on a maintainer's queue
/// signals not-ready and sits unreviewed (exactly how nushell/nushell#18549
/// was received).
fn open_pr(
    slug: &Slug,
    head_ref: &str,
    branch: &str,
    content: &PrContent,
    draft: bool,
) -> Result<()> {
    let kind = if draft { "DRAFT" } else { "ready-for-review" };
    println!(
        "{}",
        paint(
            YELLOW,
            &format!(
                "upstream-pr: opening {kind} PR upstream {}/{} <- {ORG}:{branch}...",
                slug.owner, slug.repo
            )
        )
    );
    let mut args: Vec<String> = vec![
        "pr".to_owned(),
        "create".to_owned(),
        "--repo".to_owned(),
        format!("{}/{}", slug.owner, slug.repo),
        "--base".to_owned(),
        head_ref.to_owned(),
        "--head".to_owned(),
        format!("{ORG}:{branch}"),
        "--title".to_owned(),
        content.title.clone(),
    ];
    if draft {
        args.push("--draft".to_owned());
    }
    args.extend(["--body".to_owned(), content.body.clone()]);
    let created = cmd::run("gh", &args)?;
    println!("{created}");
    Ok(())
}

/// Ensure `indexable-inc/<repo>` exists as a fork of the upstream; create it
/// (non-cloning) if absent. Idempotent.
fn ensure_fork(slug: &Slug) -> Result<()> {
    let exists = cmd::complete("gh", &["repo", "view", &format!("{ORG}/{}", slug.repo)])?;
    if exists.ok() {
        return Ok(());
    }
    println!(
        "upstream-pr: forking {}/{} into {ORG} once...",
        slug.owner, slug.repo
    );
    cmd::run(
        "gh",
        &[
            "repo",
            "fork",
            &format!("{}/{}", slug.owner, slug.repo),
            "--org",
            ORG,
            "--clone=false",
        ],
    )?;
    Ok(())
}
