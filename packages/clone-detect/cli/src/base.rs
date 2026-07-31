//! Base-tree scan for the diff gate's base-awareness (#3455).
//!
//! The diff gate must charge its budget only for duplication that is NEW
//! relative to the diff base: a tree-wide reformat touches hundreds of lines
//! inside pre-existing clone fragments without creating any duplication, and
//! counting those forces budget exceptions. To know what already existed, this
//! module checks the merge-base commit out into a temporary git worktree, runs
//! the same scan+detect pipeline over it, and collects every fragment that
//! participated in a surviving clone group there, as fingerprint
//! multiplicities plus line spans ([`BaseFragments`]) keyed in current-tree
//! coordinates so the gate can match them directly.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use clone_detect::DetectionResult;
use snafu::ResultExt as _;

use crate::{
    diff::{self, DiffError},
    gate::BaseFragments,
};

/// The base scan drops `min_lines` to 1 regardless of the configured
/// threshold: a clone pair packed below the reporting threshold at the base (a
/// one-line function body) crosses it through a pure reflow, and charging
/// those lines as new duplication would defeat reformat-safety. Over-reporting
/// at the base only ever excuses more, never charges more (#3455).
const BASE_MIN_LINES: usize = 1;

#[derive(Debug, snafu::Snafu)]
pub enum BaseError {
    #[snafu(display("failed to create a temporary directory for the base tree"))]
    TempDir { source: std::io::Error },

    #[snafu(display("failed to materialize the base tree"))]
    Git { source: DiffError },

    #[snafu(display(
        "scan target {target:?} lies outside the repository at {root:?}, \
         so it has no counterpart in the base tree"
    ))]
    TargetOutsideRepo { target: PathBuf, root: PathBuf },

    #[snafu(display(
        "base-tree fragment {fragment:?} lies outside the base worktree at {base_root:?}"
    ))]
    FragmentOutsideBase {
        fragment: PathBuf,
        base_root: PathBuf,
    },

    #[snafu(display("temporary path {path:?} is not valid UTF-8, git cannot receive it"))]
    NonUtf8TempPath { path: PathBuf },
}

/// Every fragment participating in a surviving clone group at `base_sha`,
/// keyed by the file's canonical path in the CURRENT tree (the coordinates
/// [`crate::gate::DiffGate::evaluate`] compares in).
///
/// `detect` is the caller's full scan+detect+ignore pipeline parameterized
/// over `min_lines` (relaxed to [`BASE_MIN_LINES`] here), so the base tree is
/// otherwise measured under exactly the configuration the current tree was.
pub fn preexisting_fragments(
    repo_dir: &Path,
    scan_target: &Path,
    base_sha: &str,
    detect: &dyn Fn(&Path, usize) -> Result<DetectionResult, crate::RunError>,
) -> Result<BaseFragments, crate::RunError> {
    let root = diff::repo_root(repo_dir)
        .context(GitSnafu)
        .context(crate::BaseSnafu)?;

    // The scan target's repo-relative location, so the base scan covers the
    // same subtree the current scan did.
    let target = std::fs::canonicalize(scan_target).unwrap_or_else(|_| scan_target.to_path_buf());
    let rel_target = target
        .strip_prefix(&root)
        .map_err(|_| BaseError::TargetOutsideRepo {
            target: target.clone(),
            root: root.clone(),
        })
        .context(crate::BaseSnafu)?
        .to_path_buf();

    let worktree = BaseWorktree::add(repo_dir, base_sha).context(crate::BaseSnafu)?;
    // Canonicalize before deriving fragment paths from it: on macOS the temp
    // dir is reached through a symlink (`/tmp`, `/var`), and the scanner's
    // canonicalized fragment paths would not strip against the raw spelling.
    let base_root = std::fs::canonicalize(&worktree.tree)
        .context(TempDirSnafu)
        .context(crate::BaseSnafu)?;

    let result = detect(&base_root.join(&rel_target), BASE_MIN_LINES)?;

    // Distinct fragments only: one fragment can appear in several groups, and
    // the multiplicity must count copies in the code, not group memberships.
    let mut distinct: BTreeMap<PathBuf, BTreeSet<(usize, u64)>> = BTreeMap::new();
    let mut spans: BTreeMap<PathBuf, BTreeSet<(usize, usize)>> = BTreeMap::new();
    for group in &result.instances {
        for fragment in &group.fragments {
            let file =
                std::fs::canonicalize(&fragment.file).unwrap_or_else(|_| fragment.file.clone());
            let rel = file
                .strip_prefix(&base_root)
                .map_err(|_| BaseError::FragmentOutsideBase {
                    fragment: file.clone(),
                    base_root: base_root.clone(),
                })
                .context(crate::BaseSnafu)?;
            let key = diff::absolutize(&root, rel);
            distinct
                .entry(key.clone())
                .or_default()
                .insert((fragment.byte_range.start, fragment.fingerprint));
            // Fragment rows are tree-sitter's 0-indexed coordinate; spans are
            // stored 1-indexed to match git's hunk lines (`HunkOrigin`).
            spans
                .entry(key)
                .or_default()
                .insert((fragment.lines.start + 1, fragment.lines.end + 1));
        }
    }

    let mut base = BaseFragments::default();
    for (file, fragments) in distinct {
        let counts = base.counts.entry(file).or_default();
        for (_, fingerprint) in fragments {
            *counts.entry(fingerprint).or_default() += 1;
        }
    }
    base.spans = spans
        .into_iter()
        .map(|(file, spans)| (file, spans.into_iter().collect()))
        .collect();
    Ok(base)
}

/// A detached temporary worktree of one commit. Dropping it unregisters the
/// worktree from the repository; the owned temp dir then removes the files.
struct BaseWorktree {
    repo: PathBuf,
    tree: PathBuf,
    _dir: tempfile::TempDir,
}

impl BaseWorktree {
    fn add(repo_dir: &Path, sha: &str) -> Result<Self, BaseError> {
        let dir = tempfile::Builder::new()
            .prefix("clone-diff-base-")
            .tempdir()
            .context(TempDirSnafu)?;
        let tree = dir.path().join("tree");
        let tree_str = tree
            .to_str()
            .ok_or_else(|| BaseError::NonUtf8TempPath { path: tree.clone() })?;
        // `--detach` so no branch is created or moved in the caller's repo.
        diff::git(repo_dir, &["worktree", "add", "--detach", tree_str, sha]).context(GitSnafu)?;
        Ok(Self {
            repo: repo_dir.to_path_buf(),
            tree,
            _dir: dir,
        })
    }
}

impl Drop for BaseWorktree {
    fn drop(&mut self) {
        // Unregister from `.git/worktrees`; the temp dir handles the files. A
        // failure here cannot fail the gate (the measurement already
        // happened), but it must be visible: a stale registration lingers
        // until `git worktree prune`.
        if let Some(tree) = self.tree.to_str()
            && let Err(error) = diff::git(&self.repo, &["worktree", "remove", "--force", tree])
        {
            tracing::warn!(%error, tree, "failed to remove the base-tree worktree");
        }
    }
}
