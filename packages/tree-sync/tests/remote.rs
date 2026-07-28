//! The ssh path, driven through a stand-in for ssh.
//!
//! `tree-sync` reaches a remote by handing one shell command to `ssh`. Which
//! program that is comes from `TREE_SYNC_SSH`, so these tests substitute a
//! script that runs the same command on this machine instead. Everything under
//! test is real: the generated `tar -x` and `find` and `xargs` commands, the
//! archive on the wire, and the delete list. Only the hop is removed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use tree_sync::transfer::{self, Remote};
use tree_sync::tree;

mod support;
use support::git;

/// Write an executable stand-in for ssh: it takes the last argument, which is
/// the command `tree-sync` wants run on the far end, and runs it here.
fn ssh_stand_in(dir: &Path) -> String {
    let path = dir.join("fake-ssh");
    std::fs::write(
        &path,
        "#!/bin/sh\nfor last; do :; done\nexec sh -c \"$last\"\n",
    )
    .expect("script written");
    let mut mode = std::fs::metadata(&path).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
    std::fs::set_permissions(&path, mode).expect("chmod");
    path.to_str().expect("utf-8 path").to_owned()
}

fn remote(dir: &Path) -> Remote {
    Remote {
        program: ssh_stand_in(dir),
        host: "not-a-real-host".to_owned(),
        options: Vec::new(),
    }
}

fn source_repo(root: &Path) {
    std::fs::create_dir_all(root.join("src")).expect("dirs");
    std::fs::create_dir_all(root.join("target/debug")).expect("dirs");
    std::fs::write(root.join(".gitignore"), "/target\n").expect("write");
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write");
    std::fs::write(root.join("src/lib.rs"), "pub fn helper() {}\n").expect("write");
    std::fs::write(root.join("target/debug/artifact"), "binary\n").expect("write");
    std::os::unix::fs::symlink("src/main.rs", root.join("entry")).expect("symlink");
    git(root, &["init", "--initial-branch", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "initial"]);
}

/// The whole remote push: `mkdir -p`, a real tar stream, and a real extract.
#[test]
fn a_remote_push_lands_the_git_file_set_and_nothing_else() {
    let scratch = TempDir::new().expect("tempdir");
    let source = scratch.path().join("source");
    std::fs::create_dir_all(&source).expect("dirs");
    source_repo(&source);

    // Deliberately absent, so the generated `mkdir -p` is exercised too.
    let dest = scratch.path().join("far/end");
    let remote = remote(scratch.path());

    let listing = tree::list(&source).expect("lists");
    let moved = remote
        .push(&source, &listing.entries, &dest, false, false)
        .expect("pushes");

    assert_eq!(moved.files, listing.entries.len());
    assert!(dest.join("src/main.rs").is_file());
    assert!(dest.join("src/lib.rs").is_file());
    assert_eq!(
        std::fs::read_link(dest.join("entry")).expect("symlink survived"),
        PathBuf::from("src/main.rs"),
        "a symlink must arrive as a symlink, not as a copy of its target"
    );
    assert!(
        !dest.join("target").exists(),
        "gitignored build output must not cross the wire"
    );
    assert!(
        !dest.join(".git").exists(),
        "git's own storage must not cross the wire"
    );
}

/// A second push of an unchanged tree either sends nothing, or says out loud
/// that it could not tell. The disjunction is the point: the destination's
/// `find` may lack GNU's `-printf` (macOS does), and the tool has to report that
/// rather than skip files it could not compare.
#[test]
fn a_repeat_remote_push_is_either_empty_or_says_why_not() {
    let scratch = TempDir::new().expect("tempdir");
    let source = scratch.path().join("source");
    std::fs::create_dir_all(&source).expect("dirs");
    source_repo(&source);
    let dest = scratch.path().join("far/end");
    let remote = remote(scratch.path());

    let listing = tree::list(&source).expect("lists");
    remote
        .push(&source, &listing.entries, &dest, false, false)
        .expect("first push");
    let second = remote
        .push(&source, &listing.entries, &dest, false, false)
        .expect("second push");

    if second.manifest_unavailable {
        assert_eq!(
            second.files,
            listing.entries.len(),
            "an unreadable destination must send everything, not a subset"
        );
    } else {
        // The symlink always resends; nothing else should.
        assert!(
            second.files <= 1,
            "unchanged regular files were resent: {second:?}"
        );
    }
}

/// `--delete` over ssh removes exactly the stale paths, through a `cd` into the
/// destination and a NUL-separated list, so nothing outside it is reachable.
#[test]
fn a_remote_delete_removes_only_stale_paths() {
    let scratch = TempDir::new().expect("tempdir");
    let source = scratch.path().join("source");
    std::fs::create_dir_all(&source).expect("dirs");
    source_repo(&source);
    let dest = scratch.path().join("far/end");
    let remote = remote(scratch.path());

    let listing = tree::list(&source).expect("lists");
    remote
        .push(&source, &listing.entries, &dest, false, false)
        .expect("pushes");

    std::fs::create_dir_all(dest.join("stale")).expect("dirs");
    std::fs::write(dest.join("stale/old.rs"), "left over\n").expect("write");
    let outside = scratch.path().join("bystander.txt");
    std::fs::write(&outside, "must survive\n").expect("write");

    let keep: HashSet<PathBuf> = listing
        .entries
        .iter()
        .map(|entry| entry.relative.clone())
        .collect();
    let Some(present) = remote.manifest(&dest).expect("lists destination") else {
        // No GNU find here, so there is nothing to plan a deletion from. The
        // CLI refuses --delete in exactly this case rather than guessing.
        return;
    };
    let doomed = transfer::plan_deletions(&present, &keep).expect("plans");
    assert_eq!(doomed, vec![PathBuf::from("stale/old.rs")]);

    remote.delete(&dest, &doomed, false).expect("deletes");
    assert!(!dest.join("stale/old.rs").exists());
    assert!(dest.join("src/main.rs").is_file());
    assert!(outside.is_file(), "a path outside the destination was hit");
}

/// A destination listing that climbs out of its own root is refused before any
/// removal runs.
#[test]
fn a_remote_delete_refuses_a_path_outside_the_destination() {
    let scratch = TempDir::new().expect("tempdir");
    let dest = scratch.path().join("far/end");
    std::fs::create_dir_all(&dest).expect("dirs");
    let hostage = scratch.path().join("hostage.txt");
    std::fs::write(&hostage, "must survive\n").expect("write");
    let remote = remote(scratch.path());

    let error = remote
        .delete(&dest, &[PathBuf::from("../../hostage.txt")], false)
        .expect_err("refuses");
    assert!(
        error.to_string().contains("must not climb out"),
        "unexpected error: {error}"
    );
    assert!(hostage.is_file());
}
