//! The two shapes that made rsync dangerous here, exercised against real git
//! repositories: a tree whose `.git` is a file, and an exclude written for a
//! top level build artifact.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use tree_sync::filter::Filter;
use tree_sync::transfer;
use tree_sync::tree::{self, Origin};

mod support;
use support::{git, sample_repo, write};

fn relatives(listing: &tree::Listing) -> HashSet<PathBuf> {
    listing
        .entries
        .iter()
        .map(|entry| entry.relative.clone())
        .collect()
}

/// A linked worktree's `.git` is a regular file holding a `gitdir:` line, not a
/// directory. Deciding "is this a git tree?" by looking for a `.git` directory
/// therefore reports no, falls back to a plain walk, and syncs everything git
/// would have skipped, `target/` included.
#[test]
fn a_linked_worktree_is_still_a_git_tree() {
    let repo = TempDir::new().expect("tempdir");
    sample_repo(repo.path());

    let worktrees = TempDir::new().expect("tempdir");
    let linked = worktrees.path().join("feature");
    git(
        repo.path(),
        &[
            "worktree",
            "add",
            linked.to_str().expect("utf-8 path"),
            "-b",
            "feature",
        ],
    );

    let dot_git = linked.join(".git");
    assert!(
        dot_git.is_file(),
        "the premise of this test is that a worktree's .git is a file"
    );
    assert!(
        tree::is_git_tree(&linked),
        "a linked worktree must be recognised as a git tree, or the file set \
         silently falls back to a walk that includes ignored build output"
    );

    let listing = tree::list(&linked).expect("lists");
    assert_eq!(listing.origin, Origin::Git);

    let paths = relatives(&listing);
    assert!(paths.contains(Path::new("src/main.rs")), "{paths:?}");
    assert!(
        !paths.iter().any(|path| path.starts_with("target")),
        "gitignored build output leaked into the file set: {paths:?}"
    );
    assert!(
        !paths.contains(Path::new(".git")),
        "git's own pointer file must never be synced: {paths:?}"
    );
}

/// A submodule's `.git` is a file too, and its contents are a second index that
/// the outer `git ls-files` reports only as a single gitlink entry.
#[test]
fn submodule_contents_are_listed_and_its_git_file_is_not() {
    let inner = TempDir::new().expect("tempdir");
    write(&inner.path().join("lib.rs"), "pub fn helper() {}\n");
    git(inner.path(), &["init", "--initial-branch", "main"]);
    git(inner.path(), &["add", "-A"]);
    git(inner.path(), &["commit", "-m", "inner"]);

    let outer = TempDir::new().expect("tempdir");
    sample_repo(outer.path());
    git(
        outer.path(),
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            inner.path().to_str().expect("utf-8 path"),
            "vendor/helper",
        ],
    );

    assert!(
        outer.path().join("vendor/helper/.git").is_file(),
        "the premise of this test is that a submodule's .git is a file"
    );

    let paths = relatives(&tree::list(outer.path()).expect("lists"));
    assert!(
        paths.contains(Path::new("vendor/helper/lib.rs")),
        "submodule contents must be synced, not just the gitlink: {paths:?}"
    );
    assert!(
        !paths.contains(Path::new("vendor/helper/.git")),
        "the submodule's gitdir pointer must not be synced: {paths:?}"
    );
    assert!(
        !paths.contains(Path::new("vendor/helper")),
        "the gitlink itself is a directory, not a file to send: {paths:?}"
    );
}

/// Outside git entirely, `.git` may still be a leftover file. The walk skips it
/// by name so a stale `gitdir:` pointer never reaches the destination.
#[test]
fn the_walk_fallback_skips_a_git_file() {
    let dir = TempDir::new().expect("tempdir");
    write(&dir.path().join("notes.md"), "not a repo\n");
    write(&dir.path().join(".git"), "gitdir: /gone\n");

    let listing = tree::list(dir.path()).expect("lists");
    assert_eq!(listing.origin, Origin::Walk);

    let paths = relatives(&listing);
    assert!(paths.contains(Path::new("notes.md")), "{paths:?}");
    assert!(
        !paths.contains(Path::new(".git")),
        "a leftover .git file must not be synced: {paths:?}"
    );
}

/// End to end, in the exact shape of the original incident:
/// `--exclude 'result*'` on a tree that also holds
/// `crates/codec/src/impls/result.rs`.
#[test]
fn excluding_the_result_symlink_leaves_nested_result_files_alone() {
    let repo = TempDir::new().expect("tempdir");
    sample_repo(repo.path());
    // The Nix build symlinks this repo's .gitignore does not cover, which is
    // exactly why somebody reaches for --exclude 'result*' in the first place.
    // Force-added and committed so the file set does not depend on whoever runs
    // the test: a developer's global gitignore commonly carries `result`, which
    // would drop the symlink before the exclude ever saw it.
    std::os::unix::fs::symlink("/nix/store/fake", repo.path().join("result")).expect("symlink");
    std::os::unix::fs::symlink("/nix/store/other", repo.path().join("result-doc"))
        .expect("symlink");
    git(repo.path(), &["add", "-f", "result", "result-doc"]);
    git(repo.path(), &["commit", "-m", "nix build outputs"]);

    let dest = TempDir::new().expect("tempdir");
    let listing = tree::list(repo.path()).expect("lists");
    let mut excludes =
        Filter::new(repo.path(), &["result*".to_owned()], &[]).expect("patterns build");
    let selected: Vec<tree::Entry> = listing
        .entries
        .into_iter()
        .filter(|entry| !excludes.excludes(&entry.relative))
        .collect();

    transfer::push_local(repo.path(), &selected, dest.path(), false, false).expect("copies");

    assert!(
        dest.path()
            .join("crates/codec/src/impls/result.rs")
            .is_file(),
        "the nested result.rs is what rsync silently deleted"
    );
    assert!(dest.path().join("src/main.rs").is_file());
    for excluded in ["result", "result-doc"] {
        assert!(
            dest.path().join(excluded).symlink_metadata().is_err(),
            "the top level {excluded} symlink is what the exclude named"
        );
    }
    assert!(
        !dest.path().join("target").exists(),
        "gitignored build output must not be copied"
    );

    let rules = excludes.rules();
    assert_eq!(rules[0].effective, "/result*");
    assert_eq!(
        rules[0].hits, 2,
        "only the two top level symlinks were excluded"
    );
}

/// `--delete` removes what the source no longer has, and nothing else.
#[test]
fn delete_removes_only_stale_destination_files() {
    let repo = TempDir::new().expect("tempdir");
    sample_repo(repo.path());

    let dest = TempDir::new().expect("tempdir");
    write(&dest.path().join("stale/old.rs"), "left over\n");

    let listing = tree::list(repo.path()).expect("lists");
    transfer::push_local(repo.path(), &listing.entries, dest.path(), false, false).expect("copies");

    let keep: HashSet<PathBuf> = listing
        .entries
        .iter()
        .map(|entry| entry.relative.clone())
        .collect();
    let present = transfer::local_manifest(dest.path()).expect("lists destination");
    let doomed = transfer::plan_deletions(&present, &keep).expect("plans");
    assert_eq!(doomed, vec![PathBuf::from("stale/old.rs")]);

    transfer::delete_local(dest.path(), &doomed, false).expect("deletes");
    assert!(!dest.path().join("stale/old.rs").exists());
    assert!(dest.path().join("src/main.rs").is_file());
}

/// A second run of an unchanged tree sends nothing.
#[test]
fn an_unchanged_tree_resends_nothing() {
    let repo = TempDir::new().expect("tempdir");
    sample_repo(repo.path());
    let dest = TempDir::new().expect("tempdir");

    let listing = tree::list(repo.path()).expect("lists");
    let first =
        transfer::push_local(repo.path(), &listing.entries, dest.path(), false, false).expect("copies");
    assert_eq!(first.files, listing.entries.len());
    assert_eq!(first.unchanged, 0);

    let second =
        transfer::push_local(repo.path(), &listing.entries, dest.path(), false, false).expect("copies");
    assert_eq!(second.files, 0, "nothing changed, so nothing should move");
    assert_eq!(second.unchanged, listing.entries.len());
}
