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

    ahead
        .into_iter()
        .map(|commit| {
            let changed_files = changed_files(repo, &commit)?;
            Ok(AheadCommit {
                commit,
                changed_files,
            })
        })
        .collect()
}

/// Paths that differ between `base` and `head` trees, used by the `diff`
/// subcommand. `head` accepts `"HEAD"` for the current head.
pub fn diff_stat_files(repo: &Repository, base: &str, head: &str) -> Result<Vec<String>> {
    let base_commit = repo
        .find_commit(target_oid(&resolve_ref(repo, base)?, base)?)
        .wrap_err("failed to load the base commit")?;
    let head_commit = repo
        .find_commit(target_oid(&resolve_ref(repo, head)?, head)?)
        .wrap_err("failed to load the head commit")?;

    let base_tree = base_commit.tree().wrap_err("failed to read base tree")?;
    let head_tree = head_commit.tree().wrap_err("failed to read head tree")?;

    let mut options = DiffOptions::new();
    let diff = repo
        .diff_tree_to_tree(Some(&base_tree), Some(&head_tree), Some(&mut options))
        .wrap_err("failed to diff base against head")?;

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
