//! Tree walk tests. No mount, no privileges, any platform.

use std::sync::Arc;

use jj_lib::backend::CommitId;
use jj_lib::backend::MillisSinceEpoch;
use jj_lib::backend::Timestamp;
use jj_lib::merged_tree::MergedTree;
use jj_lib::repo::Repo as _;
use jj_vfs::EntryKind;
use jj_vfs::ROOT_INODE;
use jj_vfs::TreeSnapshot;
use jj_vfs::default_materialize_options;
use pollster::FutureExt as _;
use pretty_assertions::assert_eq;
use testutils::TestRepo;
use testutils::TestThreeWayMergeTreeBuilder;
use testutils::TestTreeBuilder;
use testutils::repo_path;
use testutils::repo_path_buf;

/// Fixed so that nothing in these tests depends on the clock.
const TEST_TIME: Timestamp = Timestamp {
    timestamp: MillisSinceEpoch(1_769_000_000_000),
    tz_offset: 0,
};

fn snapshot(tree: &MergedTree) -> TreeSnapshot {
    let options = default_materialize_options(tree.store().merge_options().clone());
    TreeSnapshot::new(tree, options, &TEST_TIME, 1 << 20)
        .block_on()
        .expect("snapshot of a fresh tree")
}

/// Names in a directory, in the order readdir hands them out.
fn names(snapshot: &TreeSnapshot, inode: u64) -> Vec<String> {
    snapshot
        .readdir(inode)
        .block_on()
        .expect("readdir")
        .into_iter()
        .map(|entry| entry.name)
        .collect()
}

fn read_all(snapshot: &TreeSnapshot, inode: u64) -> Vec<u8> {
    let size = snapshot.getattr(inode).block_on().expect("getattr").size;
    let (data, eof) = snapshot
        .read(inode, 0, u32::try_from(size).expect("test file is small"))
        .block_on()
        .expect("read");
    assert!(eof, "a read of the whole file must report EOF");
    data
}

fn lookup_path(snapshot: &TreeSnapshot, path: &str) -> u64 {
    let mut inode = ROOT_INODE;
    for component in path.split('/') {
        inode = snapshot
            .lookup(inode, component)
            .block_on()
            .unwrap_or_else(|err| panic!("lookup {component} in {path}: {err}"))
            .inode;
    }
    inode
}

#[test]
fn test_readdir_is_sorted_and_recurses() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("zebra"), "z\n");
    builder.file(repo_path("apple"), "a\n");
    builder.file(repo_path("dir/nested/deep.txt"), "deep\n");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    assert_eq!(names(&snapshot, ROOT_INODE), ["apple", "dir", "zebra"]);

    let dir = lookup_path(&snapshot, "dir");
    assert_eq!(names(&snapshot, dir), ["nested"]);
    let deep = lookup_path(&snapshot, "dir/nested/deep.txt");
    assert_eq!(read_all(&snapshot, deep), b"deep\n");
}

#[test]
fn test_inodes_are_stable_across_calls() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("a"), "a\n");
    builder.file(repo_path("dir/b"), "b\n");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    let first: Vec<u64> = snapshot
        .readdir(ROOT_INODE)
        .block_on()
        .expect("readdir")
        .into_iter()
        .map(|entry| entry.inode)
        .collect();
    // Walk into the subdirectory in between, so that inode allocation happens
    // between the two listings of the root.
    let dir = lookup_path(&snapshot, "dir");
    drop(names(&snapshot, dir));
    let second: Vec<u64> = snapshot
        .readdir(ROOT_INODE)
        .block_on()
        .expect("readdir")
        .into_iter()
        .map(|entry| entry.inode)
        .collect();
    assert_eq!(first, second);

    // And a lookup agrees with what readdir said.
    assert_eq!(
        snapshot.lookup(ROOT_INODE, "a").block_on().unwrap().inode,
        first[0]
    );
    // Nothing is ever inode 0, which both FUSE and NFSv3 reserve.
    assert!(first.iter().all(|&inode| inode > ROOT_INODE));
}

#[test]
fn test_executable_bit_and_symlink() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder
        .file(repo_path("script.sh"), "#!/bin/sh\necho hi\n")
        .executable(true);
    builder.file(repo_path("plain.txt"), "plain\n");
    builder.symlink(repo_path("link"), "plain.txt");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    let script = lookup_path(&snapshot, "script.sh");
    assert_eq!(
        snapshot.getattr(script).block_on().unwrap().kind,
        EntryKind::File { executable: true }
    );
    let plain = lookup_path(&snapshot, "plain.txt");
    assert_eq!(
        snapshot.getattr(plain).block_on().unwrap().kind,
        EntryKind::File { executable: false }
    );

    let link = lookup_path(&snapshot, "link");
    let attributes = snapshot.getattr(link).block_on().unwrap();
    assert_eq!(attributes.kind, EntryKind::Symlink);
    assert_eq!(snapshot.readlink(link).block_on().unwrap(), b"plain.txt");
    // A symlink's size is the length of its target, which is what every
    // filesystem reports and what `ls -l` prints.
    assert_eq!(attributes.size, u64::try_from("plain.txt".len()).unwrap());
}

#[test]
fn test_read_at_offset_and_past_end() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("f"), "0123456789");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);
    let file = lookup_path(&snapshot, "f");

    assert_eq!(
        snapshot.read(file, 0, 4).block_on().unwrap(),
        (b"0123".to_vec(), false)
    );
    assert_eq!(
        snapshot.read(file, 6, 4).block_on().unwrap(),
        (b"6789".to_vec(), true)
    );
    // A read that runs off the end returns what is there and flags EOF, which
    // NFSv3 requires rather than treating it as an error.
    assert_eq!(
        snapshot.read(file, 8, 100).block_on().unwrap(),
        (b"89".to_vec(), true)
    );
    assert_eq!(
        snapshot.read(file, 10, 4).block_on().unwrap(),
        (Vec::new(), true)
    );
    // And well past the end, where the offset does not even fit the file.
    assert_eq!(
        snapshot.read(file, u64::MAX, 4).block_on().unwrap(),
        (Vec::new(), true)
    );
}

#[test]
fn test_missing_entries() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("dir/f"), "x\n");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    assert_eq!(
        snapshot
            .lookup(ROOT_INODE, "nope")
            .block_on()
            .unwrap_err()
            .errno(),
        libc::ENOENT
    );
    let file = lookup_path(&snapshot, "dir/f");
    // Listing a file is ENOTDIR, and reading a directory is EISDIR. Getting
    // these backwards makes shell loops behave strangely rather than fail.
    assert_eq!(
        snapshot.readdir(file).block_on().unwrap_err().errno(),
        libc::ENOTDIR
    );
    let dir = lookup_path(&snapshot, "dir");
    assert_eq!(
        snapshot.read(dir, 0, 1).block_on().unwrap_err().errno(),
        libc::EISDIR
    );
    // readlink on a regular file is EINVAL, per readlink(2).
    assert_eq!(
        snapshot.readlink(file).block_on().unwrap_err().errno(),
        libc::EINVAL
    );
    assert_eq!(
        snapshot.getattr(9999).block_on().unwrap_err().errno(),
        libc::ENOENT
    );
}

#[test]
fn test_submodule_is_an_empty_directory() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("f"), "x\n");
    builder.submodule(
        repo_path("sub"),
        CommitId::from_hex("0123456789abcdef0123456789abcdef01234567"),
    );
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    assert_eq!(names(&snapshot, ROOT_INODE), ["f", "sub"]);
    let sub = lookup_path(&snapshot, "sub");
    assert_eq!(
        snapshot.getattr(sub).block_on().unwrap().kind,
        EntryKind::Directory
    );
    assert_eq!(names(&snapshot, sub), Vec::<String>::new());
}

#[test]
fn test_conflicted_file_reads_back_as_conflict_markers() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store().clone();
    let mut builder = TestThreeWayMergeTreeBuilder::new(store);
    builder.base().file(repo_path("f"), "base\n");
    builder.parent1().file(repo_path("f"), "left\n");
    builder.parent2().file(repo_path("f"), "right\n");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    let entries = snapshot.readdir(ROOT_INODE).block_on().expect("readdir");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "f");
    assert!(
        entries[0].conflicted,
        "the entry must be flagged as conflicted"
    );
    assert_eq!(entries[0].kind, EntryKind::File { executable: false });

    // The bytes are the same conflict-marker text jj would write into a working
    // copy, so a tool reading the mount sees what a jj user already expects.
    let content = String::from_utf8(read_all(&snapshot, entries[0].inode)).expect("utf-8");
    assert!(
        content.contains("<<<<<<<"),
        "no conflict marker in:\n{content}"
    );
    assert!(
        content.contains(">>>>>>>"),
        "no conflict marker in:\n{content}"
    );
    assert!(content.contains("left"), "missing a side in:\n{content}");
    assert!(content.contains("right"), "missing a side in:\n{content}");
    // getattr must agree with what a read returns, or every caller truncates.
    assert_eq!(
        snapshot.getattr(entries[0].inode).block_on().unwrap().size,
        u64::try_from(content.len()).unwrap()
    );
}

#[test]
fn test_trivially_resolvable_path_in_a_conflicted_tree_is_not_conflicted() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store().clone();
    let mut builder = TestThreeWayMergeTreeBuilder::new(store);
    // "same" is untouched on both sides, so it merges trivially even though the
    // tree as a whole is conflicted. Reporting it as conflicted would show
    // marker text for a file nobody touched.
    builder.base().file(repo_path("same"), "same\n");
    builder.parent1().file(repo_path("same"), "same\n");
    builder.parent2().file(repo_path("same"), "same\n");
    builder.base().file(repo_path("differs"), "base\n");
    builder.parent1().file(repo_path("differs"), "left\n");
    builder.parent2().file(repo_path("differs"), "right\n");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    let entries = snapshot.readdir(ROOT_INODE).block_on().expect("readdir");
    let same = entries
        .iter()
        .find(|e| e.name == "same")
        .expect("same is listed");
    let differs = entries
        .iter()
        .find(|e| e.name == "differs")
        .expect("differs is listed");
    assert!(!same.conflicted);
    assert!(differs.conflicted);
    assert_eq!(read_all(&snapshot, same.inode), b"same\n");
}

#[test]
fn test_conflict_between_file_and_symlink_is_described() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store().clone();
    let mut builder = TestThreeWayMergeTreeBuilder::new(store);
    builder.base().file(repo_path("f"), "base\n");
    builder.parent1().file(repo_path("f"), "left\n");
    builder.parent2().symlink(repo_path("f"), "somewhere");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    let entries = snapshot.readdir(ROOT_INODE).block_on().expect("readdir");
    assert_eq!(entries.len(), 1);
    // A conflict whose sides are not all files has no marker representation, so
    // it is served as a regular file holding jj's own human summary. The
    // alternative, refusing the path with EIO, would make the whole directory
    // unusable to any recursive tool.
    assert_eq!(entries[0].kind, EntryKind::File { executable: false });
    assert!(entries[0].conflicted);
    let content = String::from_utf8(read_all(&snapshot, entries[0].inode)).expect("utf-8");
    assert!(
        content.contains("Conflict"),
        "unexpected description:\n{content}"
    );
}

#[test]
fn test_conflicted_executable_bit() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store().clone();
    let mut builder = TestThreeWayMergeTreeBuilder::new(store);
    // The executable bit is the same on every side, so it survives the content
    // conflict and the materialized file is still executable.
    builder
        .base()
        .file(repo_path("f"), "base\n")
        .executable(true);
    builder
        .parent1()
        .file(repo_path("f"), "left\n")
        .executable(true);
    builder
        .parent2()
        .file(repo_path("f"), "right\n")
        .executable(true);
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    let entries = snapshot.readdir(ROOT_INODE).block_on().expect("readdir");
    assert_eq!(entries[0].kind, EntryKind::File { executable: true });
}

#[test]
fn test_empty_tree() {
    let test_repo = TestRepo::init();
    let tree = MergedTree::resolved(
        test_repo.repo.store().clone(),
        test_repo.repo.store().empty_tree_id().clone(),
    );
    let options = default_materialize_options(tree.store().merge_options().clone());
    let snapshot = TreeSnapshot::new(&tree, options, &TEST_TIME, 1 << 20)
        .block_on()
        .expect("snapshot of the empty tree");
    assert_eq!(names(&snapshot, ROOT_INODE), Vec::<String>::new());
    assert_eq!(
        snapshot.getattr(ROOT_INODE).block_on().unwrap().kind,
        EntryKind::Directory
    );
}

#[test]
fn test_content_larger_than_the_cache_still_reads() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    let big = "x".repeat(4096);
    builder.file(repo_path("big"), &big);
    let tree = builder.write_merged_tree();
    // A budget far below the file size means the cache refuses the entry. The
    // read must still succeed, just uncached.
    let options = default_materialize_options(tree.store().merge_options().clone());
    let snapshot = TreeSnapshot::new(&tree, options, &TEST_TIME, 64)
        .block_on()
        .expect("snapshot");
    let file = lookup_path(&snapshot, "big");
    assert_eq!(snapshot.getattr(file).block_on().unwrap().size, 4096);
    assert_eq!(read_all(&snapshot, file).len(), 4096);
    // Twice, because the second read goes through the same rejected-cache path.
    assert_eq!(read_all(&snapshot, file).len(), 4096);
}

/// A `Arc<TreeSnapshot>` is what both adapters hold, so the core has to be
/// shareable across threads. This fails to compile rather than at runtime if
/// the core ever grows a non-Send field.
#[test]
fn test_snapshot_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<TreeSnapshot>>();
}

#[test]
fn test_parent_of_root_is_root() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("dir/nested/f"), "x\n");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    // POSIX says `/..` is `/`, and a client that walks up past the mount root
    // has to be told something rather than handed ENOENT.
    assert_eq!(snapshot.parent(ROOT_INODE).unwrap(), ROOT_INODE);

    let dir = lookup_path(&snapshot, "dir");
    let nested = lookup_path(&snapshot, "dir/nested");
    let file = lookup_path(&snapshot, "dir/nested/f");
    assert_eq!(snapshot.parent(file).unwrap(), nested);
    assert_eq!(snapshot.parent(nested).unwrap(), dir);
    assert_eq!(snapshot.parent(dir).unwrap(), ROOT_INODE);
    assert_eq!(snapshot.parent(9999).unwrap_err().errno(), libc::ENOENT);
}

#[test]
fn test_getattr_of_a_resolved_file_does_not_read_it() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    let big = "x".repeat(200_000);
    builder.file(repo_path("big"), &big);
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    let file = lookup_path(&snapshot, "big");
    assert_eq!(snapshot.cached_content_bytes(), 0);
    // The size comes from the store's metadata, so nothing is read and nothing
    // lands in the content cache. This is what makes `ls -l` over a directory
    // cost a listing rather than a read of every file in it.
    assert_eq!(snapshot.getattr(file).block_on().unwrap().size, 200_000);
    assert_eq!(
        snapshot.cached_content_bytes(),
        0,
        "getattr read the file instead of asking the store for its size"
    );

    // Reading is what populates the cache.
    assert_eq!(read_all(&snapshot, file).len(), 200_000);
    assert_eq!(snapshot.cached_content_bytes(), 200_000);
}

#[test]
fn test_reported_size_always_equals_bytes_read() {
    let test_repo = TestRepo::init();
    let store = test_repo.repo.store().clone();
    let mut builder = TestThreeWayMergeTreeBuilder::new(store);
    // Same on every side, so these stay resolved while "conflicted" does not.
    for side in 0..3 {
        let b = match side {
            0 => builder.base(),
            1 => builder.parent1(),
            _ => builder.parent2(),
        };
        b.file(repo_path("empty"), "");
        b.file(repo_path("small"), "abc");
        b.symlink(repo_path("link"), "small");
    }
    builder.base().file(repo_path("conflicted"), "base\n");
    builder.parent1().file(repo_path("conflicted"), "left\n");
    builder.parent2().file(repo_path("conflicted"), "right\n");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    // The invariant NixOS/nix#10667 is about: Nix writes a file into its store
    // using the size from stat and only then reads the content, so any
    // disagreement between the two silently stores the wrong bytes under a hash
    // of bytes that never existed. Check every entry, including an empty file, a
    // symlink and a conflicted path, since those take three different routes to
    // a size.
    for entry in snapshot.readdir(ROOT_INODE).block_on().expect("readdir") {
        if entry.kind == EntryKind::Directory {
            continue;
        }
        let reported = snapshot.getattr(entry.inode).block_on().unwrap().size;
        let actual = read_all(&snapshot, entry.inode).len();
        assert_eq!(
            reported,
            u64::try_from(actual).unwrap(),
            "{}: getattr said {reported} but read returned {actual}",
            entry.name
        );
    }
}

/// The journal seam exists and accepts entries, so a future write path has
/// somewhere to record without a "if journaling is configured" branch. v0
/// records nothing, so what is asserted here is the shape rather than any
/// behavior.
#[test]
fn test_null_journal_accepts_entries() {
    use std::time::SystemTime;

    use jj_vfs::journal::Actor;
    use jj_vfs::journal::ContentRef;
    use jj_vfs::journal::Entry;
    use jj_vfs::journal::Journal as _;
    use jj_vfs::journal::NullJournal;
    use jj_vfs::journal::Operation;

    let journal = NullJournal;
    journal
        .record(Entry {
            sequence: 1,
            timestamp: SystemTime::UNIX_EPOCH,
            path: repo_path_buf("f"),
            operation: Operation::Write {
                content: ContentRef::Inline(b"x".to_vec()),
            },
            // FUSE can fill this in; NFSv3 carries no caller identity, so the
            // field is optional rather than guessed. Both shapes must be
            // representable.
            actor: Some(Actor { pid: 1, uid: 501 }),
        })
        .expect("the null journal accepts anything");
    journal
        .record(Entry {
            sequence: 2,
            timestamp: SystemTime::UNIX_EPOCH,
            path: repo_path_buf("g"),
            operation: Operation::Rename {
                from: repo_path_buf("f"),
            },
            actor: None,
        })
        .expect("an entry with no attribution is still an entry");
}

/// Two names differing only in case must stay two entries.
///
/// This matters beyond tidiness. Nix's `use-case-hack` defaults on for darwin
/// and appends a `~nix~case~hack~` suffix when it sees names that collide
/// case-insensitively, so a case-folding mount would bake those mangled names
/// into the NAR and make store paths computed on a Mac diverge from Linux for
/// identical content. That is a silent cross-platform reproducibility break
/// that surfaces as an unexplainable cache miss.
///
/// The tree is built through the store rather than through a working copy on
/// purpose: this Mac's APFS is case-insensitive, so `Foo` and `foo` cannot both
/// exist as files here, and a test that went via a checkout would be untestable
/// on the platform that most needs the guarantee.
#[test]
fn test_names_differing_only_in_case_stay_distinct() {
    let test_repo = TestRepo::init();
    let mut builder = TestTreeBuilder::new(test_repo.repo.store().clone());
    builder.file(repo_path("Foo"), "upper\n");
    builder.file(repo_path("foo"), "lower\n");
    builder.file(repo_path("dir/Bar"), "upper nested\n");
    builder.file(repo_path("dir/bar"), "lower nested\n");
    let tree = builder.write_merged_tree();
    let snapshot = snapshot(&tree);

    assert_eq!(names(&snapshot, ROOT_INODE), ["Foo", "dir", "foo"]);

    let upper = lookup_path(&snapshot, "Foo");
    let lower = lookup_path(&snapshot, "foo");
    assert_ne!(
        upper, lower,
        "Foo and foo must have distinct inodes, or a client cannot tell them apart"
    );
    assert_eq!(read_all(&snapshot, upper), b"upper\n");
    assert_eq!(read_all(&snapshot, lower), b"lower\n");

    // And in a subdirectory, since the listing is built per directory.
    let dir = lookup_path(&snapshot, "dir");
    assert_eq!(names(&snapshot, dir), ["Bar", "bar"]);
    assert_eq!(
        read_all(&snapshot, lookup_path(&snapshot, "dir/Bar")),
        b"upper nested\n"
    );
    assert_eq!(
        read_all(&snapshot, lookup_path(&snapshot, "dir/bar")),
        b"lower nested\n"
    );
}
