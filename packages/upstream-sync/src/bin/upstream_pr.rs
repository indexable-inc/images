//! `upstream-pr <pkg> <patch> [--open] [--dry-run]`: contribute ONE of our
//! fork patches upstream without carrying the rest of the series.
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
//!   4. Pushes the branch to an indexable-inc fork of the upstream repo
//!      (created with `gh repo fork --clone=false` if absent). Pushing to
//!      OUR fork is fine; it is not the outward act.
//!   5. Prints the ready-to-open compare URL. With `--open`, additionally
//!      opens a DRAFT PR upstream. Default is prepare-only: opening the
//!      upstream PR is the outward act and stays behind an explicit `--open`
//!      a human invokes.
//!
//! The PR's title and body come FROM THE PATCH ITSELF: subject = title,
//! commit message body = PR body (one fact, one home; the fork mapping
//! deliberately has no duplicate description field), plus AI attribution and
//! a link back to the patch file of record. An optional
//! `patches.<patch>.prExtra` in the mapping is appended after the body for
//! upstream-specific PR-template content (issue refs, checklists) that does
//! not belong in a commit message. A body-less commit is refused; the
//! `patch-dag-<name>` check enforces the same for every attempt-marked patch
//! so the failure happens in CI, not mid-contribution.
//!
//! `--dry-run` runs the whole flow (closure, fetch, am, branch) but skips
//! the push and PR, printing what it WOULD push. Used to validate content
//! without touching any remote.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anstream::println;
use clap::Parser;
use color_eyre::eyre::{Result, WrapErr, bail, eyre};
use lazy_regex::regex;
use upstream_sync::mapping::{self, Fork, Slug};
use upstream_sync::style::{CYAN, GREEN, YELLOW, paint};
use upstream_sync::{cmd, dag, patch};

/// The GitHub org whose forks host the contribution branches.
const ORG: &str = "indexable-inc";

#[derive(Parser)]
#[command(name = "upstream-pr")]
struct Cli {
    /// fork package name (codex | btop | clippy)
    pkg: String,
    /// patch file name (or its NNNN prefix / unique substring)
    patch: String,
    /// also open a DRAFT PR upstream (outward act; default: prepare only)
    #[arg(long)]
    open: bool,
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
        bail!("upstream-pr: {}: missing dag.json in {}; run `nix run .#rebase-patches -- dag`", cli.pkg, fork.patch_dir);
    }
    let doc = dag::Doc::load(&dag_file)?;
    let all_patches = doc.patch_names();

    // Resolve the requested patch to an exact node name (exact, then prefix,
    // then unique substring).
    let target = dag::resolve(&cli.patch, &all_patches)?;
    println!("{}", paint(CYAN, &format!("upstream-pr: {}: target patch {target}", cli.pkg)));

    // Ancestor closure from the DAG, in NNNN order, plus the target last.
    let closure = doc.closure(&target);
    let pos: HashMap<&str, usize> = all_patches.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
    let by_series = |p: &String| pos.get(p.as_str()).copied().unwrap_or(usize::MAX);
    let mut ordered = closure.clone();
    ordered.push(target.clone());
    ordered.sort_by_key(by_series);
    ordered.dedup();
    if closure.is_empty() {
        println!("upstream-pr: {}: {target} is independent; contributing it alone.", cli.pkg);
    } else {
        println!(
            "{}",
            paint(YELLOW, &format!("upstream-pr: {}: {target} is NOT independent; its upstream contribution drags {} ancestor patch(es):", cli.pkg, closure.len()))
        );
        let mut sorted = closure.clone();
        sorted.sort_by_key(by_series);
        for c in &sorted {
            println!("  - {c}");
        }
        println!("{}", paint(YELLOW, "upstream-pr: consider splitting, or send the closure as one PR."));
    }

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
    let (head_ref, tip) = prepare_branch(&scratch, &fork, &slug, &branch, &cli.pkg)?;
    apply_closure(&scratch, &patch_dir, &ordered, &tip, &cli.pkg)?;

    let n_commits = cmd::run_in(&scratch, "git", &["rev-list", "--count", &format!("{tip}..HEAD")])?;
    let tip_short: String = tip.chars().take(10).collect();
    println!(
        "{}",
        paint(GREEN, &format!("upstream-pr: {}: applied {n_commits} commit(s) cleanly onto {}/{}@{head_ref} ({tip_short})", cli.pkg, slug.owner, slug.repo))
    );

    if cli.dry_run {
        println!(
            "{}",
            paint(GREEN, &format!("upstream-pr: --dry-run: would push branch {branch} to {ORG}/{} and print a compare URL. Commits:", slug.repo))
        );
        println!("{}", cmd::run_in(&scratch, "git", &["log", "--oneline", &format!("{tip}..HEAD")])?);
        println!("upstream-pr: scratch repo left for inspection: {}", scratch.display());
        return Ok(());
    }

    // Ensure an indexable-inc fork of the upstream exists, then push.
    ensure_fork(&slug)?;
    println!("upstream-pr: pushing {branch} to {ORG}/{}...", slug.repo);
    cmd::run_in(&scratch, "git", &["remote", "add", "fork", &format!("https://github.com/{ORG}/{}.git", slug.repo)])?;
    cmd::run_in(&scratch, "git", &["push", "--force", "fork", &branch])?;

    let compare = format!(
        "https://github.com/{}/{}/compare/{head_ref}...{ORG}:{}:{branch}?expand=1",
        slug.owner, slug.repo, slug.repo
    );
    println!("{}", paint(GREEN, &format!("upstream-pr: {}: pushed. Ready-to-open compare URL:", cli.pkg)));
    println!("  {compare}");

    if cli.open {
        open_draft_pr(&cli.pkg, &fork, &scratch, &slug, &head_ref, &branch, &target)?;
    } else {
        println!("upstream-pr: prepare-only. Re-run with `--open` to open a DRAFT PR upstream, or open the compare URL by hand.");
    }

    fs::remove_dir_all(&scratch).wrap_err_with(|| format!("cannot remove scratch repo {}", scratch.display()))?;
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
/// check out the contribution branch at its tip. Returns (head_ref, tip).
fn prepare_branch(scratch: &Path, fork: &Fork, slug: &Slug, branch: &str, pkg: &str) -> Result<(String, String)> {
    cmd::run_in(scratch, "git", &["init", "--quiet"])?;
    neutralize_config(scratch)?;
    println!("upstream-pr: fetching {}/{} default branch tip...", slug.owner, slug.repo);
    cmd::run_in(scratch, "git", &["remote", "add", "upstream", &fork.url])?;

    // Discover the default branch (HEAD) of upstream, then fetch just it.
    let symref = cmd::run_in(scratch, "git", &["ls-remote", "--symref", "upstream", "HEAD"])?;
    let head_ref = symref
        .lines()
        .find(|l| l.starts_with("ref:"))
        .and_then(|l| regex!(r"ref:\s+refs/heads/(\S+)\s+HEAD").captures(l))
        .map(|c| c[1].to_owned())
        .ok_or_else(|| eyre!("upstream-pr: {pkg}: cannot discover the default branch of {}", fork.url))?;
    println!("upstream-pr: upstream default branch is {head_ref}");

    cmd::run_in(scratch, "git", &["fetch", "--quiet", "upstream", &head_ref])?;
    let tip = cmd::run_in(scratch, "git", &["rev-parse", "FETCH_HEAD"])?;
    cmd::run_in(scratch, "git", &["checkout", "--quiet", "-b", branch, &tip])?;
    Ok((head_ref, tip))
}

/// Apply the closure onto the tip with 3-way. On conflict, fail loudly: this
/// is where our old base drifting from the upstream tip shows up.
fn apply_closure(scratch: &Path, patch_dir: &Path, ordered: &[String], tip: &str, pkg: &str) -> Result<()> {
    let mut am_args: Vec<String> = vec!["am".to_owned(), "--3way".to_owned()];
    am_args.extend(ordered.iter().map(|p| patch_dir.join(p).display().to_string()));
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
        format!("conflicting files: [{}]", unmerged.lines().collect::<Vec<_>>().join(", "))
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
        println!("{}", paint(YELLOW, "upstream-pr: no `origin` remote here; the PR body will omit the patch-of-record link."));
        return Ok(None);
    }
    let url = res.stdout.trim();
    let Some(caps) = regex!(r"github\.com[:/]([^/]+)/([^/]+?)(\.git)?$").captures(url) else {
        println!(
            "{}",
            paint(YELLOW, &format!("upstream-pr: origin {url} is not a parseable github URL; the PR body will omit the patch-of-record link."))
        );
        return Ok(None);
    };
    Ok(Some(format!("https://github.com/{}/{}/blob/main/{patch_dir_rel}/{patch}", &caps[1], &caps[2])))
}

/// The outward act, gated behind --open. Draft only. Title and body come
/// from the patch's own commit message (one fact, one home: nix carries no
/// duplicate description field), so a body-less commit is refused loudly;
/// the `patch-dag-<name>` check enforces the same for attempt-marked patches
/// before it ever gets here.
fn open_draft_pr(pkg: &str, fork: &Fork, scratch: &Path, slug: &Slug, head_ref: &str, branch: &str, target: &str) -> Result<()> {
    let title = cmd::run_in(scratch, "git", &["log", "-1", "--format=%s", "HEAD"])?;
    let commit_body = cmd::run_in(scratch, "git", &["log", "-1", "--format=%b", "HEAD"])?;
    if commit_body.is_empty() {
        bail!("upstream-pr: {pkg}: {target} has no commit-message body; write the why in the commit body (it becomes the upstream PR description).");
    }

    // Optional upstream-specific PR-template content (issue refs,
    // checklists) that does not belong in a commit message, declared as
    // `patches.<patch>.prExtra` in the fork mapping.
    let pr_extra = fork.patches.get(target).and_then(|m| m.pr_extra.clone());
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
    let mut parts = vec![commit_body];
    parts.extend(pr_extra);
    parts.push(attribution);
    let body = parts.join("\n\n");

    println!(
        "{}",
        paint(YELLOW, &format!("upstream-pr: opening DRAFT PR upstream {}/{} <- {ORG}:{branch}...", slug.owner, slug.repo))
    );
    let created = cmd::run(
        "gh",
        &[
            "pr",
            "create",
            "--repo",
            &format!("{}/{}", slug.owner, slug.repo),
            "--base",
            head_ref,
            "--head",
            &format!("{ORG}:{branch}"),
            "--title",
            &title,
            "--draft",
            "--body",
            &body,
        ],
    )?;
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
    println!("upstream-pr: forking {}/{} into {ORG} once...", slug.owner, slug.repo);
    cmd::run("gh", &["repo", "fork", &format!("{}/{}", slug.owner, slug.repo), "--org", ORG, "--clone=false"])?;
    Ok(())
}
