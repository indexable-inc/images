//! Writable overlay tests. No mount, no privileges, any unix.
//!
//! These drive [`OverlayTree`] directly rather than through a transport,
//! because the invariant being defended lives in the core: the revision's set
//! of names is exactly the set of names you see, minus nothing. A transport can
//! only fail to report that correctly, and `test_nfs` covers the reporting.

use std::path::PathBuf;
use std::sync::Arc;

use jj_lib::backend::MillisSinceEpoch;
use jj_lib::backend::Timestamp;
use jj_lib::merged_tree::MergedTree;
use jj_lib::repo::Repo as _;
use jj_vfs::EntryKind;
use jj_vfs::Overlay;
use jj_vfs::OverlayTree;
use jj_vfs::ROOT_INODE;
use jj_vfs::SnapshotError;
use jj_vfs::TreeSnapshot;
use jj_vfs::default_materialize_options;
use pollster::FutureExt as _;
use pretty_assertions::assert_eq;
use testutils::TestRepo;
use testutils::TestTreeBuilder;
use testutils::repo_path;

/// Fixed so that nothing here depends on the clock.
const TEST_TIME: Timestamp = Timestamp {
    timestamp: MillisSinceEpoch(1_769_000_000_000),
    tz_offset: 0,
};

/// Stands in for a tree key where the test is about the lock rather than the
/// revision the layer is bound to.
const REVISION: &str = "test-revision";

fn snapshot(tree: &MergedTree) -> Arc<TreeSnapshot> {
    let options = default_materialize_options(tree.store().merge_options().clone());
    Arc::new(
        TreeSnapshot::new(tree, options, &TEST_TIME, 1 << 20)
            .block_on()
            .expect("snapshot of a fresh tree"),
    )
}

/// A tree holding one tracked file, one tracked directory with a file in it,
/// and a writable layer in a fresh temporary directory.
///
/// The temporary directory is returned so the caller can keep it alive;
/// dropping it deletes the upper layer out from under the mount.
fn fixture() -> (OverlayTree, tempfile::TempDir) {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("bun.lock"), "tracked lockfile\n");
    builder.file(repo_path("src/main.rs"), "fn main() {}\n");
    let tree = builder.write_merged_tree();
    let upper = tempfile::tempdir().expect("a temporary directory");
    (writable(&tree, upper.path().join("upper")), upper)
}

/// A writable tree over `tree`, with its scratch layer at `upper`.
fn writable(tree: &MergedTree, upper: PathBuf) -> OverlayTree {
    let lower = snapshot(tree);
    let overlay = Overlay::open(upper, &lower.tree_key()).expect("open the writable layer");
    OverlayTree::writable(lower, overlay)
}

fn lookup(tree: &OverlayTree, path: &str) -> u64 {
    let mut inode = ROOT_INODE;
    for component in path.split('/') {
        inode = tree
            .lookup(inode, component)
            .block_on()
            .unwrap_or_else(|err| panic!("lookup {path} at component {component}: {err}"))
            .inode;
    }
    inode
}

fn read_all(tree: &OverlayTree, inode: u64) -> Vec<u8> {
    let size = tree.getattr(inode).block_on().expect("getattr").size;
    let (data, eof) = tree
        .read(inode, 0, u32::try_from(size).expect("test file is small"))
        .block_on()
        .expect("read");
    assert!(eof, "a read of the whole file must report EOF");
    data
}

fn names(tree: &OverlayTree, inode: u64) -> Vec<String> {
    tree.readdir(inode)
        .block_on()
        .expect("readdir")
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

#[test]
fn test_a_mount_without_an_overlay_refuses_every_write() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("a.txt"), "a\n");
    let tree = OverlayTree::read_only(snapshot(&builder.write_merged_tree()));

    // Not "no overlay configured, so writes go nowhere". Each of these has to
    // be an error the caller can see, since the previous behavior of this
    // mount was to refuse writes and that must not have regressed.
    let created = tree.mkdir(ROOT_INODE, "node_modules").block_on();
    assert!(
        matches!(created, Err(SnapshotError::ReadOnly)),
        "mkdir on a read-only mount returned {created:?}"
    );
    let removed = tree.remove(ROOT_INODE, "a.txt").block_on();
    assert!(
        matches!(removed, Err(SnapshotError::ReadOnly)),
        "remove on a read-only mount returned {removed:?}"
    );
    assert_eq!(SnapshotError::ReadOnly.errno(), libc::EROFS);
}

#[test]
fn test_creating_a_directory_the_revision_does_not_have() {
    // The reported failure: `bun install` cannot make a node_modules.
    let (tree, _upper) = fixture();
    let created = tree
        .mkdir(ROOT_INODE, "node_modules")
        .block_on()
        .expect("mkdir a name the revision does not contain");
    assert_eq!(created.kind, EntryKind::Directory);

    let file = tree
        .create(created.inode, "installed.txt", None)
        .block_on()
        .expect("create inside the new directory");
    tree.write(file.inode, 0, b"from bun\n")
        .block_on()
        .expect("write");
    assert_eq!(read_all(&tree, file.inode), b"from bun\n");

    // The union: what the revision has, plus what was just made.
    assert_eq!(
        names(&tree, ROOT_INODE),
        vec!["bun.lock", "node_modules", "src"]
    );
    assert_eq!(names(&tree, created.inode), vec!["installed.txt"]);
}

#[test]
fn test_writing_a_tracked_file_copies_it_up_and_keeps_its_inode() {
    let (tree, _upper) = fixture();
    let inode = lookup(&tree, "bun.lock");
    assert_eq!(read_all(&tree, inode), b"tracked lockfile\n");

    tree.truncate(inode, 0).block_on().expect("truncate");
    tree.write(inode, 0, b"rewritten by bun\n")
        .block_on()
        .expect("write to a tracked file");

    assert_eq!(read_all(&tree, inode), b"rewritten by bun\n");
    // The inode has to survive copy-up. An NFS file handle is built from it,
    // and a client that opened the file before the write would get ESTALE on
    // its own file if the number moved underneath it.
    assert_eq!(lookup(&tree, "bun.lock"), inode);
    // Still one entry, not two.
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "src"]);
}

#[test]
fn test_copy_up_does_not_touch_the_revision() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("bun.lock"), "tracked lockfile\n");
    let merged = builder.write_merged_tree();
    let upper = tempfile::tempdir().expect("a temporary directory");
    let tree = writable(&merged, upper.path().join("upper"));

    let inode = lookup(&tree, "bun.lock");
    tree.truncate(inode, 0).block_on().expect("truncate");
    tree.write(inode, 0, b"overlay only\n")
        .block_on()
        .expect("write");

    // A second, independent view of the same revision with no overlay at all.
    // If copy-up had written through to the store this would see the change.
    let untouched = OverlayTree::read_only(snapshot(&merged));
    let same_path = lookup(&untouched, "bun.lock");
    assert_eq!(read_all(&untouched, same_path), b"tracked lockfile\n");
}

#[test]
fn test_removing_a_tracked_file_hides_it() {
    let (tree, _upper) = fixture();
    tree.remove(ROOT_INODE, "bun.lock")
        .block_on()
        .expect("remove a tracked file");

    // Gone from both places a name can turn into an entry. Checking only one
    // of them is how a name comes back in a listing but not a lookup, or the
    // other way round, which is worse than not deleting it at all.
    assert_eq!(names(&tree, ROOT_INODE), vec!["src"]);
    let looked_up = tree.lookup(ROOT_INODE, "bun.lock").block_on();
    assert!(
        matches!(looked_up, Err(SnapshotError::NotFound)),
        "looking up a deleted tracked file returned {looked_up:?}"
    );

    // A second remove is ENOENT, not a second whiteout, because as far as the
    // caller can tell the name is already gone.
    let again = tree.remove(ROOT_INODE, "bun.lock").block_on();
    assert!(
        matches!(again, Err(SnapshotError::NotFound)),
        "removing a name twice returned {again:?}"
    );
}

#[test]
fn test_removing_a_tracked_file_that_was_copied_up_hides_it_too() {
    // Two states reach the same place: a tracked name with a real file behind
    // it in the scratch layer, and one with nothing. Only the first has
    // anything to unlink, and forgetting the second is how a delete reports
    // success and the file comes back.
    let (tree, _upper) = fixture();
    let inode = lookup(&tree, "bun.lock");
    tree.write(inode, 0, b"scratch\n")
        .block_on()
        .expect("write");
    tree.remove(ROOT_INODE, "bun.lock")
        .block_on()
        .expect("remove a copied-up tracked file");
    assert_eq!(names(&tree, ROOT_INODE), vec!["src"]);
}

#[test]
fn test_removing_a_tracked_directory_is_refused() {
    // The line the module draws. Hiding a directory hides a subtree, which
    // needs opaque directories and readdir subtraction below the whiteout;
    // hiding a file needs one name in one set.
    let (tree, _upper) = fixture();
    let removed = tree.remove(ROOT_INODE, "src").block_on();
    assert!(
        matches!(&removed, Err(SnapshotError::Tracked { path, .. }) if path == "src"),
        "removing a tracked directory returned {removed:?}"
    );

    // So `rm -rf src` fails, but at the rmdir rather than at the first file.
    tree.remove(lookup(&tree, "src"), "main.rs")
        .block_on()
        .expect("remove a tracked file inside a tracked directory");
    assert_eq!(names(&tree, lookup(&tree, "src")), Vec::<String>::new());
    let still_refused = tree.remove(ROOT_INODE, "src").block_on();
    assert!(
        matches!(still_refused, Err(SnapshotError::Tracked { .. })),
        "removing an emptied tracked directory returned {still_refused:?}"
    );
    assert_eq!(
        SnapshotError::Tracked {
            operation: "remove the directory",
            path: String::new()
        }
        .errno(),
        libc::EROFS
    );
}

#[test]
fn test_deleting_a_tracked_file_does_not_touch_the_revision() {
    // The property that makes whiteouts safe to have at all: the deletion
    // lives in the scratch layer, so discarding the scratch layer is a
    // complete undo and the store never hears about it.
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("bun.lock"), "tracked lockfile\n");
    let merged = builder.write_merged_tree();
    let upper = tempfile::tempdir().expect("a temporary directory");
    let tree = writable(&merged, upper.path().join("upper"));
    tree.remove(ROOT_INODE, "bun.lock")
        .block_on()
        .expect("remove");

    let untouched = OverlayTree::read_only(snapshot(&merged));
    assert_eq!(names(&untouched, ROOT_INODE), vec!["bun.lock"]);
    assert_eq!(
        read_all(&untouched, lookup(&untouched, "bun.lock")),
        b"tracked lockfile\n"
    );
}

#[test]
fn test_recreating_a_deleted_tracked_name_clears_the_whiteout() {
    let (tree, _upper) = fixture();
    tree.remove(ROOT_INODE, "bun.lock")
        .block_on()
        .expect("remove");
    let created = tree
        .create(ROOT_INODE, "bun.lock", None)
        .block_on()
        .expect("create over a deleted tracked name");
    tree.write(created.inode, 0, b"new\n")
        .block_on()
        .expect("write");

    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "src"]);
    assert_eq!(read_all(&tree, lookup(&tree, "bun.lock")), b"new\n");

    // And deleting it again hides it again rather than leaving the upper file
    // to be found by the next lookup.
    tree.remove(ROOT_INODE, "bun.lock")
        .block_on()
        .expect("remove the recreated name");
    assert_eq!(names(&tree, ROOT_INODE), vec!["src"]);
}

#[test]
fn test_removing_an_untracked_path_works() {
    // The other half of the same rule: `rm -rf node_modules` has to work, or
    // every package manager's cleanup step fails.
    let (tree, _upper) = fixture();
    let directory = tree
        .mkdir(ROOT_INODE, "node_modules")
        .block_on()
        .expect("mkdir");
    let file = tree
        .create(directory.inode, "junk", None)
        .block_on()
        .expect("create");
    drop(file);

    let too_early = tree.remove(ROOT_INODE, "node_modules").block_on();
    assert!(
        matches!(too_early, Err(SnapshotError::NotEmpty { .. })),
        "removing a non-empty directory returned {too_early:?}"
    );

    tree.remove(directory.inode, "junk")
        .block_on()
        .expect("remove the file");
    tree.remove(ROOT_INODE, "node_modules")
        .block_on()
        .expect("remove the now-empty directory");
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "src"]);
}

#[test]
fn test_write_to_temp_then_rename_over_a_tracked_file() {
    // The idiom almost every careful writer uses, and the one the narrower
    // "only untracked paths" rule could not support: the temp name is allowed
    // and then the rename onto the tracked name has to be too, or nothing that
    // updates a lockfile works.
    let (tree, _upper) = fixture();
    let temporary = tree
        .create(ROOT_INODE, "bun.lock.tmp", None)
        .block_on()
        .expect("create the temp file");
    tree.write(temporary.inode, 0, b"freshly resolved\n")
        .block_on()
        .expect("write the temp file");
    tree.rename(ROOT_INODE, "bun.lock.tmp", ROOT_INODE, "bun.lock")
        .block_on()
        .expect("rename over a tracked file");

    assert_eq!(
        read_all(&tree, lookup(&tree, "bun.lock")),
        b"freshly resolved\n"
    );
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "src"]);
}

#[test]
fn test_renaming_a_tracked_file_away_carries_its_content() {
    // The source has never been written to, so there is no scratch file to
    // move and the revision's content has to be copied up first. Getting this
    // wrong renames nothing and reports success.
    let (tree, _upper) = fixture();
    tree.rename(ROOT_INODE, "bun.lock", ROOT_INODE, "bun.lock.old")
        .block_on()
        .expect("rename a tracked file away");
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock.old", "src"]);
    assert_eq!(
        read_all(&tree, lookup(&tree, "bun.lock.old")),
        b"tracked lockfile\n"
    );
}

#[test]
fn test_renaming_a_tracked_directory_away_is_refused() {
    let (tree, _upper) = fixture();
    let renamed = tree
        .rename(ROOT_INODE, "src", ROOT_INODE, "src.old")
        .block_on();
    assert!(
        matches!(&renamed, Err(SnapshotError::Tracked { path, .. }) if path == "src"),
        "renaming a tracked directory away returned {renamed:?}"
    );
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "src"]);
}

#[test]
fn test_the_lockfile_replacement_that_bun_actually_does() {
    // The command that prompted the whole feature. `bun install` writes the
    // new lockfile to a temporary name, renames the tracked one aside, and
    // renames the new one into place. Every step of that, in order, with no
    // refusal anywhere.
    let (tree, _upper) = fixture();
    let temporary = tree
        .create(ROOT_INODE, "bun.lock.tmp", None)
        .block_on()
        .expect("create the temporary");
    tree.write(temporary.inode, 0, b"resolved\n")
        .block_on()
        .expect("write the new lockfile");
    tree.rename(ROOT_INODE, "bun.lock", ROOT_INODE, "bun.lock.old")
        .block_on()
        .expect("rename the old lockfile aside");
    tree.rename(ROOT_INODE, "bun.lock.tmp", ROOT_INODE, "bun.lock")
        .block_on()
        .expect("rename the new lockfile into place");
    tree.remove(ROOT_INODE, "bun.lock.old")
        .block_on()
        .expect("drop the old lockfile");

    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "src"]);
    assert_eq!(read_all(&tree, lookup(&tree, "bun.lock")), b"resolved\n");
}

#[test]
fn test_shadowing_a_tracked_directory_is_refused() {
    // A file where a tracked directory is would make everything underneath it
    // unreachable through the readdir union, which is a deletion wearing a
    // create's clothes.
    let (tree, _upper) = fixture();
    let created = tree.create(ROOT_INODE, "src", None).block_on();
    assert!(
        matches!(&created, Err(SnapshotError::Tracked { path, .. }) if path == "src"),
        "creating a file over a tracked directory returned {created:?}"
    );

    let scratch = tree
        .create(ROOT_INODE, "scratch", None)
        .block_on()
        .expect("create");
    drop(scratch);
    let renamed = tree
        .rename(ROOT_INODE, "scratch", ROOT_INODE, "src")
        .block_on();
    assert!(
        matches!(&renamed, Err(SnapshotError::Tracked { path, .. }) if path == "src"),
        "renaming a file over a tracked directory returned {renamed:?}"
    );

    // The directory and its contents are still there.
    assert_eq!(names(&tree, lookup(&tree, "src")), vec!["main.rs"]);
}

#[test]
fn test_creating_inside_a_tracked_directory() {
    // `cargo build` writing a target/ inside a tracked crate directory, and the
    // case that proves an upper container never hides the lower directory it
    // mirrors.
    let (tree, _upper) = fixture();
    let src = lookup(&tree, "src");
    tree.create(src, "generated.rs", None)
        .block_on()
        .expect("create inside a tracked directory");
    assert_eq!(names(&tree, src), vec!["generated.rs", "main.rs"]);
    assert_eq!(
        read_all(&tree, lookup(&tree, "src/main.rs")),
        b"fn main() {}\n"
    );
}

#[test]
fn test_the_scratch_layer_survives_a_remount() {
    // Persistence is not a nicety. A `bun install` that has to run again on
    // every mount makes the feature not worth using.
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("bun.lock"), "tracked lockfile\n");
    let merged = builder.write_merged_tree();
    let scratch = tempfile::tempdir().expect("a temporary directory");
    let upper_root = scratch.path().join("upper");

    {
        let tree = writable(&merged, upper_root.clone());
        let directory = tree
            .mkdir(ROOT_INODE, "node_modules")
            .block_on()
            .expect("mkdir");
        let file = tree
            .create(directory.inode, "left.txt", None)
            .block_on()
            .expect("create");
        tree.write(file.inode, 0, b"still here\n")
            .block_on()
            .expect("write");
    }

    // A second mount, a fresh snapshot, the same scratch directory.
    let tree = writable(&merged, upper_root);
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "node_modules"]);
    assert_eq!(
        read_all(&tree, lookup(&tree, "node_modules/left.txt")),
        b"still here\n"
    );
}

#[test]
fn test_a_deleted_tracked_file_stays_deleted_across_a_remount() {
    // A deletion the caller has to redo on every mount is not a deletion, and
    // the scratch layer already persists everything else about the mount.
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("bun.lock"), "tracked lockfile\n");
    builder.file(repo_path("src/main.rs"), "fn main() {}\n");
    let merged = builder.write_merged_tree();
    let scratch = tempfile::tempdir().expect("a temporary directory");
    let upper_root = scratch.path().join("upper");

    {
        let tree = writable(&merged, upper_root.clone());
        tree.remove(ROOT_INODE, "bun.lock")
            .block_on()
            .expect("remove");
    }

    let tree = writable(&merged, upper_root);
    assert_eq!(names(&tree, ROOT_INODE), vec!["src"]);
    let looked_up = tree.lookup(ROOT_INODE, "bun.lock").block_on();
    assert!(
        matches!(looked_up, Err(SnapshotError::NotFound)),
        "a whiteout did not survive the remount: {looked_up:?}"
    );
}

#[test]
fn test_a_whiteout_does_not_carry_to_a_different_revision() {
    // The question the initial change left open. Deleting `bun.lock` says
    // something about the file *this* revision has at that name; it says
    // nothing about a different revision's file. Erring toward showing the
    // name is the only safe direction, since the other one hides a file the
    // caller never asked to hide.
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store().clone();
    let mut builder = TestTreeBuilder::new(store.clone());
    builder.file(repo_path("bun.lock"), "first\n");
    let first = builder.write_merged_tree();
    let mut builder = TestTreeBuilder::new(store);
    builder.file(repo_path("bun.lock"), "second\n");
    let second = builder.write_merged_tree();

    let scratch = tempfile::tempdir().expect("a temporary directory");
    let upper_root = scratch.path().join("upper");
    {
        let tree = writable(&first, upper_root.clone());
        tree.mkdir(ROOT_INODE, "node_modules")
            .block_on()
            .expect("mkdir");
        tree.remove(ROOT_INODE, "bun.lock")
            .block_on()
            .expect("remove");
        assert_eq!(names(&tree, ROOT_INODE), vec!["node_modules"]);
    }

    // Same scratch directory, different revision. The installed tree persists,
    // because it is about paths. The deletion does not, because it was about
    // the first revision's file.
    let tree = writable(&second, upper_root.clone());
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "node_modules"]);
    assert_eq!(read_all(&tree, lookup(&tree, "bun.lock")), b"second\n");
    drop(tree);

    // And going back is not a way to recover the whiteout: the log belonging
    // to the first revision was discarded when the second one was mounted.
    let tree = writable(&first, upper_root);
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "node_modules"]);
}

#[test]
fn test_a_whiteout_survives_a_newline_in_the_name() {
    // The whiteout log is line-oriented and a filesystem name is not. A name
    // containing a newline ends its record early; a name *ending* in a
    // carriage return loses it to `str::lines`, which strips one as part of a
    // `\r\n` pair. Either way the record that comes back names the neighbor
    // rather than the file that was deleted. Cheap now, a mystery later.
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("od\nd"), "awkward\n");
    builder.file(repo_path("oc\r"), "also awkward\n");
    builder.file(repo_path("oc"), "innocent\n");
    builder.file(repo_path("od"), "innocent\n");
    let merged = builder.write_merged_tree();
    let scratch = tempfile::tempdir().expect("a temporary directory");
    let upper_root = scratch.path().join("upper");

    {
        let tree = writable(&merged, upper_root.clone());
        tree.remove(ROOT_INODE, "od\nd").block_on().expect("remove");
        tree.remove(ROOT_INODE, "oc\r").block_on().expect("remove");
        assert_eq!(names(&tree, ROOT_INODE), vec!["oc", "od"]);
    }

    // Both innocents still here. A record that ended early would have hidden
    // `od` or `oc` instead of the awkward name beside it.
    let tree = writable(&merged, upper_root);
    assert_eq!(names(&tree, ROOT_INODE), vec!["oc", "od"]);
}

#[test]
fn test_the_whiteout_log_does_not_grow_without_bound() {
    // `bun install` deletes and recreates one lockfile, and a watch-and-build
    // loop does it once per build. An append-only log with no compaction turns
    // that into a file that grows forever inside a directory nobody looks at.
    let (tree, upper) = fixture();
    for _ in 0..200 {
        tree.remove(ROOT_INODE, "bun.lock")
            .block_on()
            .expect("remove");
        tree.create(ROOT_INODE, "bun.lock", None)
            .block_on()
            .expect("create");
    }
    let log = std::fs::read_to_string(upper.path().join("upper").join(jj_vfs::WHITEOUT_NAME))
        .expect("read the whiteout log");
    let lines = log.lines().count();
    assert!(
        lines <= 70,
        "400 operations left {lines} lines in the whiteout log"
    );
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "src"]);
}

#[test]
fn test_the_whiteout_log_is_not_served() {
    // Same reason the lock file is not: a name in the listing that belongs to
    // no revision, and whose deletion would take the mount's deletions with
    // it.
    let (tree, _upper) = fixture();
    tree.remove(ROOT_INODE, "bun.lock")
        .block_on()
        .expect("remove");
    assert_eq!(names(&tree, ROOT_INODE), vec!["src"]);
    let looked_up = tree.lookup(ROOT_INODE, jj_vfs::WHITEOUT_NAME).block_on();
    assert!(
        matches!(looked_up, Err(SnapshotError::NotFound)),
        "the whiteout log resolved through the mount: {looked_up:?}"
    );
}

#[test]
fn test_two_mounts_cannot_share_one_scratch_layer() {
    // A `jj fs mount` can outlive its own mountpoint, so a user remounting the
    // same path can end up with two servers running. Without this they would be
    // two writers on one directory with nothing between them.
    let scratch = tempfile::tempdir().expect("a temporary directory");
    let root = scratch.path().join("upper");
    let first = Overlay::open(root.clone(), REVISION).expect("the first open takes the lock");
    let second = Overlay::open(root.clone(), REVISION);
    assert!(
        matches!(second, Err(SnapshotError::OverlayBusy { .. })),
        "a second open of the same layer returned {second:?}"
    );

    // Released when the holder goes away, so a clean unmount does not strand
    // the layer.
    drop(first);
    Overlay::open(root, REVISION).expect("open again once the first is gone");
}

#[test]
fn test_an_overlay_file_reports_its_own_mtime() {
    // Every incremental build system decides what to rebuild by comparing
    // timestamps. Reporting the commit's timestamp for a file written seconds
    // ago makes them skip work they have to do.
    let (tree, _upper) = fixture();
    let file = tree
        .create(ROOT_INODE, "built.o", None)
        .block_on()
        .expect("create");
    let written = tree.getattr(file.inode).block_on().expect("getattr").mtime;
    let tracked = tree
        .getattr(lookup(&tree, "src/main.rs"))
        .block_on()
        .expect("getattr")
        .mtime;
    assert!(
        written > tracked,
        "a file just written reports {written:?}, no newer than the commit's {tracked:?}"
    );
}

#[test]
fn test_symlinks_in_the_scratch_layer() {
    // node_modules is mostly symlinks under any workspace-aware package
    // manager, so this is the common case rather than an edge one.
    let (tree, _upper) = fixture();
    let link = tree
        .symlink(ROOT_INODE, "link", b"src/main.rs")
        .block_on()
        .expect("symlink");
    assert_eq!(link.kind, EntryKind::Symlink);
    assert_eq!(
        tree.readlink(link.inode).block_on().expect("readlink"),
        b"src/main.rs"
    );
    // Refused at a name that is already taken, which is what symlink(2) does.
    let clash = tree
        .symlink(ROOT_INODE, "bun.lock", b"elsewhere")
        .block_on();
    assert!(
        matches!(clash, Err(SnapshotError::Exists { .. })),
        "symlink over an existing name returned {clash:?}"
    );
}

#[test]
fn test_the_lock_file_is_not_served() {
    // It belongs to the mount, not to the user. A listing containing it would
    // offer a name that no revision has and whose deletion breaks the mount.
    let (tree, _upper) = fixture();
    tree.create(ROOT_INODE, "visible.txt", None)
        .block_on()
        .expect("create");
    let listed = names(&tree, ROOT_INODE);
    assert!(
        !listed.iter().any(|name| name.starts_with(".jj-overlay")),
        "the lock file appeared in a listing: {listed:?}"
    );
    assert_eq!(listed, vec!["bun.lock", "src", "visible.txt"]);
}

#[test]
fn test_appledouble_sidecars_are_accepted_and_discarded() {
    // macOS has no other way to store `com.apple.provenance` over NFSv3, so it
    // writes a 4.1 KB `._name` file beside every real one. One `bun install`
    // produced 39,003 of them for 38,517 real entries. They are transport
    // artifact, not the caller's data.
    let (tree, upper) = fixture();
    let sidecar = tree
        .create(ROOT_INODE, "._bun.lock", None)
        .block_on()
        .expect("a sidecar create must be accepted, not refused");
    let written = tree
        .write(sidecar.inode, 0, &[0u8; 4096])
        .block_on()
        .expect("a sidecar write must be accepted");
    assert_eq!(written, 4096, "the client has to be told its write landed");

    // Accepted, and stored nowhere.
    let host = upper.path().join("upper").join("._bun.lock");
    assert!(
        !host.exists(),
        "a discarded sidecar must not reach the disk: {}",
        host.display()
    );

    // Never listed, so it cannot pollute a directory the user reads.
    let listed = names(&tree, ROOT_INODE);
    assert_eq!(listed, vec!["bun.lock", "src"]);

    // Reads back as empty, which is how macOS learns there is no xattr.
    assert_eq!(read_all(&tree, sidecar.inode), Vec::<u8>::new());

    // And the real file beside it is untouched.
    assert_eq!(
        read_all(&tree, lookup(&tree, "bun.lock")),
        b"tracked lockfile\n"
    );
}

#[test]
fn test_a_sidecar_that_was_never_created_is_a_miss() {
    // The client has to be able to discover that it needs to create one.
    let (tree, _upper) = fixture();
    let missing = tree.lookup(ROOT_INODE, "._nothing").block_on();
    assert!(
        matches!(missing, Err(SnapshotError::NotFound)),
        "an uncreated sidecar returned {missing:?}"
    );
}

#[test]
fn test_a_sidecar_follows_its_file_through_a_rename() {
    // macOS renames the sidecar alongside the real file. Refusing that would
    // fail the rename of a file that has nothing wrong with it.
    let (tree, _upper) = fixture();
    tree.create(ROOT_INODE, "._scratch", None)
        .block_on()
        .expect("create the sidecar");
    tree.rename(ROOT_INODE, "._scratch", ROOT_INODE, "._renamed")
        .block_on()
        .expect("rename the sidecar");
    assert!(tree.lookup(ROOT_INODE, "._renamed").block_on().is_ok());
    assert!(tree.lookup(ROOT_INODE, "._scratch").block_on().is_err());
    // Still invisible under either name.
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "src"]);
}

#[test]
fn test_readdirplus_can_describe_overlay_entries_without_reading_them() {
    // A directory listing that carries no attributes costs the client one round
    // trip per entry afterwards. Measured on one `bun install`: 17,552 listings
    // followed by 3,374,005 GETATTR calls, 62.5% of all traffic. An entry in the
    // writable layer is a real file, so its attributes are one `lstat`, and
    // carrying them is the entire reason READDIRPLUS exists.
    let (tree, _upper) = fixture();
    let created = tree
        .create(ROOT_INODE, "built.o", None)
        .block_on()
        .expect("create");
    tree.write(created.inode, 0, b"0123456789")
        .block_on()
        .expect("write");

    let described = tree
        .cheap_getattr(created.inode)
        .block_on()
        .expect("an overlay entry must be describable without reading it");
    assert_eq!(described.size, 10);

    // An entry that still lives only in the revision has no size until its
    // content is read, which is the case the read-only mount deliberately
    // refused to pay for. It stays undescribed rather than becoming slow.
    let tracked = lookup(&tree, "bun.lock");
    assert!(
        tree.cheap_getattr(tracked).block_on().is_none(),
        "a revision-only entry must not be described cheaply"
    );
}

#[test]
fn test_sidecars_already_on_disk_are_hidden() {
    // A scratch layer created before this accommodation existed holds thousands
    // of them. They should stop appearing, not start being served.
    let (tree, upper) = fixture();
    let root = upper.path().join("upper");
    std::fs::write(root.join("._stale"), b"left by an older mount").expect("write a stale sidecar");
    assert_eq!(names(&tree, ROOT_INODE), vec!["bun.lock", "src"]);
}

/// Size an inode reports, which is the thing an attribute cache can get wrong.
fn reported_size(tree: &OverlayTree, inode: u64) -> u64 {
    tree.getattr(inode).block_on().expect("getattr").size
}

#[test]
fn test_cached_attributes_follow_every_mutation() {
    // The writable layer answers `getattr` from memory rather than an `lstat`,
    // because it is the only writer and already knows. That is only true while
    // every mutation updates or drops what it knows, and a stale size here is
    // invisible to a test that only ever writes once. So: mutate every way
    // there is and check the reported size after each.
    let (tree, _upper) = fixture();
    let file = tree
        .create(ROOT_INODE, "artifact.bin", None)
        .block_on()
        .expect("create");
    assert_eq!(reported_size(&tree, file.inode), 0);

    // Extending write.
    tree.write(file.inode, 0, b"0123456789")
        .block_on()
        .expect("write");
    assert_eq!(reported_size(&tree, file.inode), 10);

    // A write inside the existing extent must not shrink it.
    tree.write(file.inode, 0, b"ab").block_on().expect("write");
    assert_eq!(reported_size(&tree, file.inode), 10);

    // A write past the end extends it.
    tree.write(file.inode, 20, b"zz").block_on().expect("write");
    assert_eq!(reported_size(&tree, file.inode), 22);

    // Truncate shrinks it, which a max-only cache would miss.
    tree.truncate(file.inode, 4).block_on().expect("truncate");
    assert_eq!(reported_size(&tree, file.inode), 4);
    // "0123456789" with "ab" written over the front, cut to four bytes.
    assert_eq!(read_all(&tree, file.inode), b"ab23");

    // Creating over an existing name truncates, so the cached size and any
    // held descriptor describe a file that no longer exists.
    let recreated = tree
        .create(ROOT_INODE, "artifact.bin", None)
        .block_on()
        .expect("create over an existing name");
    assert_eq!(recreated.inode, file.inode, "the inode must be reused");
    assert_eq!(reported_size(&tree, recreated.inode), 0);

    // Remove then recreate hands back the same inode, so anything remembered
    // about the old file would describe the new one.
    tree.write(recreated.inode, 0, b"0123456789")
        .block_on()
        .expect("write");
    tree.remove(ROOT_INODE, "artifact.bin")
        .block_on()
        .expect("remove");
    let third = tree
        .create(ROOT_INODE, "artifact.bin", None)
        .block_on()
        .expect("recreate");
    assert_eq!(reported_size(&tree, third.inode), 0);
}

#[test]
fn test_cached_attributes_survive_a_rename() {
    // Both ends of a rename change what they name, and the destination inode
    // may have described something else a moment earlier.
    let (tree, _upper) = fixture();
    let old = tree
        .create(ROOT_INODE, "old", None)
        .block_on()
        .expect("create");
    tree.write(old.inode, 0, b"0123456789")
        .block_on()
        .expect("write");
    let target = tree
        .create(ROOT_INODE, "new", None)
        .block_on()
        .expect("create");
    tree.write(target.inode, 0, b"xx")
        .block_on()
        .expect("write");
    assert_eq!(reported_size(&tree, target.inode), 2);

    tree.rename(ROOT_INODE, "old", ROOT_INODE, "new")
        .block_on()
        .expect("rename");

    // "new" now holds what "old" held. A cache that kept the old destination
    // size would report 2 for a 10 byte file.
    let renamed = lookup(&tree, "new");
    assert_eq!(reported_size(&tree, renamed), 10);
    assert_eq!(read_all(&tree, renamed), b"0123456789");
    assert!(tree.lookup(ROOT_INODE, "old").block_on().is_err());
}

#[test]
fn test_a_write_after_copy_up_reports_the_copied_size() {
    // Copy-up installs the file by renaming a temporary over it, so a
    // descriptor or size cached a moment earlier would be on the wrong file.
    let (tree, _upper) = fixture();
    let inode = lookup(&tree, "bun.lock");
    assert_eq!(reported_size(&tree, inode), 17);

    tree.write(inode, 0, b"XX").block_on().expect("write");
    // The copy is 17 bytes with the first two overwritten, not a 2 byte file.
    assert_eq!(reported_size(&tree, inode), 17);
    assert_eq!(read_all(&tree, inode), b"XXacked lockfile\n");
}
