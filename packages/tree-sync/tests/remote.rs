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

/// Every file in a macOS checkout carries at least one extended attribute
/// (`com.apple.provenance`), and bsdtar serialises xattrs as `AppleDouble`
/// sidecars: a `._<name>` companion written next to every real file. A tree
/// synced that way arrives with twice the files it should have, and the extra
/// ones are not in the repo.
///
/// `tree-sync` is immune BY CONSTRUCTION, because `write_archive` builds the
/// stream with the Rust `tar` crate rather than shelling out to the system
/// `tar`, and it writes exactly the entries it is handed. This test exists to
/// keep it that way: an "optimisation" that pipes through `/usr/bin/tar` on the
/// sending side would reintroduce the fault, and it would not be caught by any
/// other test here because every existing fixture writes files with no xattrs.
///
/// Observed cost of the fault before it was understood (ENG-11861): 14,528
/// stray files in one sync, surfacing three layers away as a nix evaluation
/// error about a patch series filename, because one of the sidecars was
/// `._0001-update-nox.patch` and a guard correctly refused it.
#[test]
fn a_file_with_extended_attributes_arrives_without_an_appledouble_sidecar() {
    let scratch = TempDir::new().expect("tempdir");
    let source = scratch.path().join("source");
    std::fs::create_dir_all(&source).expect("dirs");
    source_repo(&source);

    // Only meaningful where the platform actually has xattrs to serialise; the
    // assertion below is unconditional so a Linux CI run still guards the
    // destination, it simply cannot reproduce the source condition.
    set_xattr(&source.join("src/main.rs"));

    let dest = scratch.path().join("far/end");
    let remote = remote(scratch.path());
    let listing = tree::list(&source).expect("lists");
    remote
        .push(&source, &listing.entries, &dest, false, false)
        .expect("pushes");

    assert!(dest.join("src/main.rs").is_file(), "the real file arrived");
    assert_no_apple_double(&dest);
}

/// Best-effort: tag a file with an extended attribute on platforms that have
/// them. A failure here is not a test failure -- the destination assertion is
/// the guard, and it holds whether or not the source could be tagged.
fn set_xattr(path: &Path) {
    let _ = std::process::Command::new("xattr")
        .args(["-w", "com.apple.provenance", "tree-sync-test"])
        .arg(path)
        .status();
}

/// Fail if any `AppleDouble` sidecar reached `root`, naming the files rather than
/// leaving the next reader to decode a `._` prefix.
///
/// Named separately from the test so it can be exercised directly; see
/// `the_appledouble_assertion_actually_fires`.
fn assert_no_apple_double(root: &Path) {
    let strays = apple_double_files(root);
    assert!(
        strays.is_empty(),
        "the transport added {} AppleDouble sidecar(s) that are not in the repo: {:?}. \
         This is bsdtar serialising extended attributes; the sending side must build the \
         archive itself (see `write_archive`) or set COPYFILE_DISABLE=1.",
        strays.len(),
        strays
    );
}

fn apple_double_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if entry.file_name().to_string_lossy().starts_with("._") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The guard on the guard. `assert_no_apple_double` passing means nothing
/// unless it can fail, and a sidecar-detector that never detects one is exactly
/// the absence-shaped pass this whole ticket is about.
#[test]
fn the_appledouble_assertion_actually_fires() {
    let scratch = TempDir::new().expect("tempdir");
    let dir = scratch.path().join("planted");
    std::fs::create_dir_all(dir.join("nested")).expect("dirs");
    std::fs::write(dir.join("nested/._0001-update-nox.patch"), b"").expect("write");

    assert_eq!(
        apple_double_files(&dir).len(),
        1,
        "the detector must find a planted sidecar, including one nested below the root"
    );

    let caught = std::panic::catch_unwind(|| assert_no_apple_double(&dir));
    assert!(
        caught.is_err(),
        "assert_no_apple_double must panic when a sidecar is present"
    );
}
