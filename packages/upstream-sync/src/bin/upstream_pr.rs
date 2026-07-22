//! `upstream-pr <pkg> <patch> [--open] [--dry-run]`: contribute ONE of our
//! fork patches upstream without carrying the rest of the series.
//!
//! Each fork's patch series lives as real commits in its GitHub fork repo
//! (lib/fork-packages.nix `forkRepo`/`bookmark`, the jj megamerge layout):
//! every patch is a commit whose parents are its true dependencies. That
//! makes contribution push-only:
//!
//!   1. Resolve the requested patch (by commit SUBJECT, the identity that
//!      survives jj rebases) to its commit on the fork bookmark.
//!   2. Refuse a commit whose message body states no reason: the body
//!      becomes the upstream PR description (one fact, one home; the fork
//!      mapping deliberately has no duplicate description field). This runs
//!      BEFORE anything is pushed, in every mode including --dry-run.
//!   3. Push the commit to branch `upstream/<slug-of-subject>` on the fork
//!      repo. The commit's git ancestry IS its dependency closure, so the
//!      branch carries exactly the patches the contribution needs; when
//!      that is more than the patch itself we warn, listing the ancestors.
//!      Pushing to OUR fork is fine; it is not the outward act.
//!   4. Prints the ready-to-open compare URL. With `--open`, additionally
//!      opens a DRAFT PR upstream against its default branch, from the fork
//!      (`--head <fork-owner>:<branch>`). Default is prepare-only: opening
//!      the upstream PR is the outward act and stays behind an explicit
//!      `--open` a human invokes.
//!
//! The PR's title and body come FROM THE COMMIT ITSELF: subject = title,
//! message body = PR body, plus an optional `patches.<subject>.prExtra`
//! from the mapping (upstream-specific PR-template content: issue refs,
//! checklists), AI attribution, and a patch-of-record link to the commit in
//! the fork repo.
//!
//! `--dry-run` resolves the patch and runs the reason-of-record check but
//! pushes nothing and opens nothing, printing what it WOULD push.

use std::path::PathBuf;

use anstream::println;
use clap::Parser;
use color_eyre::eyre::{Result, bail};
use upstream_sync::mapping::{self, Fork, Slug};
use upstream_sync::style::{CYAN, GREEN, YELLOW, paint};
use upstream_sync::{cmd, series};

#[derive(Parser)]
#[command(name = "upstream-pr")]
struct Cli {
    /// fork package name (nix | btop | ...)
    pkg: String,
    /// patch commit subject (or a unique prefix / substring)
    patch: String,
    /// also open a DRAFT PR upstream (outward act; default: prepare only)
    #[arg(long)]
    open: bool,
    /// resolve + validate only; push nothing, open nothing
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

    // The gh PR path only exists for GitHub upstreams; refuse the outward
    // act up front (before any clone or push) rather than after preparing.
    if cli.open && !mapping::is_github(&fork.upstream_url) {
        bail!(
            "upstream-pr: {}: upstream {} is not GitHub; there is no gh path to open a PR there. Prepare the branch without --open and submit by hand.",
            cli.pkg,
            fork.upstream_url
        );
    }

    println!(
        "upstream-pr: {}: reading series from {}@{}...",
        cli.pkg, fork.fork_repo, fork.bookmark
    );
    let repo = series::Repo::open(&fork)?;
    let subject = series::resolve(&cli.patch, &repo.subjects())?;
    let target = repo
        .find(&subject)
        .expect("resolve returned a subject from the series")
        .clone();
    println!(
        "{}",
        paint(
            CYAN,
            &format!(
                "upstream-pr: {}: target patch '{subject}' ({})",
                cli.pkg,
                &target.sha[..12]
            )
        )
    );

    // Reason of record, checked before ANY push in every mode: the commit
    // body becomes the upstream PR description, so a body-less commit has
    // nothing to say upstream.
    let commit_body = repo.body(&target.sha)?;
    if commit_body.trim().is_empty() {
        bail!(
            "upstream-pr: {}: '{subject}' has no commit-message body; write the why in the commit body (it becomes the upstream PR description).",
            cli.pkg
        );
    }

    let closure = report_closure(&cli.pkg, &repo, &target)?;
    let branch = format!("upstream/{}", series::slug(&subject));

    if cli.dry_run {
        println!(
            "{}",
            paint(
                GREEN,
                &format!(
                    "upstream-pr: --dry-run: would push {} commit(s) to {} branch {branch} and print a compare URL. Commits:",
                    closure.len(),
                    fork.fork_repo
                )
            )
        );
        for c in &closure {
            println!("  {} {}", &c.sha[..12], c.subject);
        }
        return Ok(());
    }

    println!(
        "upstream-pr: pushing {} to {} branch {branch}...",
        &target.sha[..12],
        fork.fork_repo
    );
    repo.push_branch(&target.sha, &branch)?;

    let slug = Slug::parse(&fork.upstream_url)?;
    let fork_name = fork
        .fork_repo
        .split_once('/')
        .map_or(fork.fork_repo.as_str(), |(_, name)| name);
    let compare = format!(
        "https://github.com/{}/{}/compare/{}...{}:{fork_name}:{branch}?expand=1",
        slug.owner,
        slug.repo,
        repo.upstream_branch,
        fork.fork_owner()
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

    if cli.open {
        open_draft_pr(&fork, &repo, &slug, &branch, &target, &commit_body)?;
    } else {
        println!(
            "upstream-pr: prepare-only. Re-run with `--open` to open a DRAFT PR upstream, or open the compare URL by hand."
        );
    }
    Ok(())
}

/// The contribution closure (the commit's ancestry back to the base), with
/// a loud warning when the patch drags ancestors: the upstream PR is then
/// not single-commit, by construction.
fn report_closure(
    pkg: &str,
    repo: &series::Repo,
    target: &series::Commit,
) -> Result<Vec<series::Commit>> {
    let closure = repo.closure(&target.sha)?;
    let ancestors: Vec<&series::Commit> =
        closure.iter().filter(|c| c.sha != target.sha).collect();
    if ancestors.is_empty() {
        println!("upstream-pr: {pkg}: '{}' is independent; contributing it alone.", target.subject);
    } else {
        println!(
            "{}",
            paint(
                YELLOW,
                &format!(
                    "upstream-pr: {pkg}: '{}' is NOT independent; its upstream contribution drags {} ancestor patch(es):",
                    target.subject,
                    ancestors.len()
                )
            )
        );
        for c in &ancestors {
            println!("  - {}", c.subject);
        }
        println!(
            "{}",
            paint(
                YELLOW,
                "upstream-pr: consider splitting, or send the closure as one PR."
            )
        );
    }
    Ok(closure)
}

/// The outward act, gated behind --open. Draft only. Title and body come
/// from the patch commit's own message (one fact, one home).
fn open_draft_pr(
    fork: &Fork,
    repo: &series::Repo,
    slug: &Slug,
    branch: &str,
    target: &series::Commit,
    commit_body: &str,
) -> Result<()> {
    // Optional upstream-specific PR-template content (issue refs,
    // checklists) that does not belong in a commit message, declared as
    // `patches.<subject>.prExtra` in the fork mapping.
    let pr_extra = fork
        .patches
        .get(&target.subject)
        .and_then(|m| m.pr_extra.clone());
    // The patch of record is the commit in OUR fork repo: permanent (every
    // bookmark push pins the rev under refs/pins/), and its ancestry shows
    // the dependency closure.
    let attribution = [
        "---".to_owned(),
        format!(
            "Contributed from a maintained fork; the patch of record is https://github.com/{}/commit/{}.",
            fork.fork_repo, target.sha
        ),
        "Prepared with AI assistance (Claude); directed and reviewed by a human maintainer."
            .to_owned(),
    ]
    .join("\n\n");
    let mut parts = vec![commit_body.trim().to_owned()];
    parts.extend(pr_extra);
    parts.push(attribution);
    let body = parts.join("\n\n");

    println!(
        "{}",
        paint(
            YELLOW,
            &format!(
                "upstream-pr: opening DRAFT PR upstream {}/{} <- {}:{branch}...",
                slug.owner,
                slug.repo,
                fork.fork_owner()
            )
        )
    );
    let created = cmd::run(
        "gh",
        &[
            "pr",
            "create",
            "--repo",
            &format!("{}/{}", slug.owner, slug.repo),
            "--base",
            &repo.upstream_branch,
            "--head",
            &format!("{}:{branch}", fork.fork_owner()),
            "--title",
            &target.subject,
            "--draft",
            "--body",
            &body,
        ],
    )?;
    println!("{created}");
    Ok(())
}
