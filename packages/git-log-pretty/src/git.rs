//! Repository queries built on [`git2`]: which commits are ahead of a base, and
//! which files each commit or diff touches.

use std::collections::HashSet;

use color_eyre::eyre::{Result, WrapErr, eyre};
use git2::{Commit, DiffOptions, Oid, Repository};

/// A commit selected for display, paired with the paths it changed. The ahead
/// list is sorted newest-first by commit time.
pub struct AheadCommit<'repo> {
    pub commit: Commit<'repo>,
    pub changed_files: Vec<String>,
}

/// Open the repository containing the current directory.
pub fn discover() -> Result<Repository> {
    Repository::discover(".").wrap_err("failed to discover a git repository from the current directory")
}

/// The short name of the branch HEAD points at, or `None` when HEAD is detached
/// or otherwise not on a branch. Used to decide whether to show recent history
/// (on `main`) or the ahead-of-`main` diff (anywhere else).
pub fn head_branch_name(repo: &Repository) -> Option<String> {
    let head = repo.head().ok()?;
    head.is_branch().then(|| head.shorthand().map(str::to_string))?
}

/// Pair a commit with the files it changed, the shape the display layer expects.
fn into_ahead<'repo>(repo: &Repository, commit: Commit<'repo>) -> Result<AheadCommit<'repo>> {
    let changed_files = changed_files(repo, &commit)?;
    Ok(AheadCommit {
        commit,
        changed_files,
    })
}

/// The most recent `limit` commits reachable from HEAD, newest-first, each
/// paired with the files it changed. Used when HEAD is `main` and there is
/// nothing to be ahead of.
pub fn recent_commits(repo: &Repository, limit: usize) -> Result<Vec<AheadCommit<'_>>> {
    let mut revwalk = repo.revwalk().wrap_err("failed to start a revwalk")?;
    revwalk
        .set_sorting(git2::Sort::TIME)
        .wrap_err("failed to set revwalk sorting")?;
    revwalk.push_head().wrap_err("failed to seed the revwalk from HEAD")?;

    revwalk
        .take(limit)
        .map(|oid| {
            let oid = oid.wrap_err("failed to walk commit history")?;
            let commit = repo.find_commit(oid).wrap_err("failed to load a commit")?;
            into_ahead(repo, commit)
        })
        .collect()
}

/// Resolve the single commit id a reference points at, erroring when the ref is
/// symbolic or otherwise has no direct target.
fn target_oid(reference: &git2::Reference, label: &str) -> Result<Oid> {
    reference
        .target()
        .ok_or_else(|| eyre!("{label} does not point at a commit"))
}

/// Resolve a branch-ish name to a reference, trying a local branch, then a
/// remote-tracking branch, then the name as a fully qualified ref. `"HEAD"` is
/// resolved against the current head.
fn resolve_ref<'repo>(repo: &'repo Repository, name: &str) -> Result<git2::Reference<'repo>> {
    if name == "HEAD" {
        return repo.head().wrap_err("failed to resolve HEAD");
    }

    let candidates = [
        format!("refs/heads/{name}"),
        format!("refs/remotes/{name}"),
        // Single-branch clones (CI, fresh checkouts) often have only the feature
        // branch locally while the base lives at `origin/<name>`, so fall back to
        // the remote-tracking branch before giving up.
        format!("refs/remotes/origin/{name}"),
        name.to_string(),
    ];

    candidates
        .iter()
        .find_map(|candidate| repo.find_reference(candidate).ok())
        .ok_or_else(|| eyre!("failed to find branch or ref: {name}"))
}

/// Collect every commit reachable from `start`, used to diff two histories.
fn reachable(repo: &Repository, start: Oid) -> Result<HashSet<Oid>> {
    let mut revwalk = repo.revwalk().wrap_err("failed to start a revwalk")?;
    revwalk.push(start).wrap_err("failed to seed the revwalk")?;

    revwalk
        .collect::<std::result::Result<HashSet<Oid>, _>>()
        .wrap_err("failed to walk commit history")
}

/// Paths touched by `commit` relative to its first parent. A root commit (no
/// parent) reports every file in its tree.
pub fn changed_files(repo: &Repository, commit: &Commit) -> Result<Vec<String>> {
    let new_tree = commit.tree().wrap_err("failed to read commit tree")?;

    let old_tree = match commit.parent(0) {
        Ok(parent) => Some(parent.tree().wrap_err("failed to read parent tree")?),
        Err(_) => None,
    };

    let mut options = DiffOptions::new();
    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut options))
        .wrap_err("failed to diff commit against its parent")?;

    Ok(collect_diff_paths(&diff))
}

/// Commits reachable from HEAD but not from `base`, newest-first, each paired
/// with its changed files. An empty vec means HEAD is not ahead of `base`.
pub fn commits_ahead<'repo>(repo: &'repo Repository, base: &str) -> Result<Vec<AheadCommit<'repo>>> {
    let base_oid = target_oid(&resolve_ref(repo, base)?, base)?;
    let head_oid = target_oid(&repo.head().wrap_err("failed to resolve HEAD")?, "HEAD")?;

    if base_oid == head_oid {
        return Ok(Vec::new());
    }

    let base_set = reachable(repo, base_oid)?;
    let head_set = reachable(repo, head_oid)?;

    let mut ahead: Vec<Commit<'repo>> = head_set
        .difference(&base_set)
        .map(|oid| repo.find_commit(*oid).wrap_err("failed to load an ahead commit"))
        .collect::<Result<_>>()?;

    ahead.sort_by_key(|commit| std::cmp::Reverse(commit.time().seconds()));

    ahead.into_iter().map(|commit| into_ahead(repo, commit)).collect()
}

/// Paths `head` changed relative to where it forked from `base`, used by the
/// `diff` subcommand. This is the `base...head` (merge-base) view git's `diff`
/// advertises with the triple dot: diffing the merge base against `head` so
/// commits that landed on `base` after the fork point do not pollute the tree.
/// `head` accepts `"HEAD"` for the current head.
pub fn diff_stat_files(repo: &Repository, base: &str, head: &str) -> Result<Vec<String>> {
    let base_oid = target_oid(&resolve_ref(repo, base)?, base)?;
    let head_oid = target_oid(&resolve_ref(repo, head)?, head)?;

    let merge_base_oid = repo
        .merge_base(base_oid, head_oid)
        .wrap_err("failed to find the merge base of base and head")?;
    let merge_base_commit = repo
        .find_commit(merge_base_oid)
        .wrap_err("failed to load the merge-base commit")?;
    let head_commit = repo
        .find_commit(head_oid)
        .wrap_err("failed to load the head commit")?;

    let base_tree = merge_base_commit.tree().wrap_err("failed to read merge-base tree")?;
    let head_tree = head_commit.tree().wrap_err("failed to read head tree")?;

    let mut options = DiffOptions::new();
    let diff = repo
        .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut options))
        .wrap_err("failed to diff merge base against head")?;

    Ok(collect_diff_paths(&diff))
}

/// Pull one display path per delta, preferring the new path and falling back to
/// the old one for deletions.
fn collect_diff_paths(diff: &git2::Diff) -> Vec<String> {
    diff.deltas()
        .filter_map(|delta| {
            let path = delta.new_file().path().or_else(|| delta.old_file().path());
            path.and_then(|p| p.to_str()).map(str::to_string)
        })
        .collect()
}
