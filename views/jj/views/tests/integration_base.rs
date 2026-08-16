//! The stale outer base clobber, as tests.
//!
//! A lifted merge commit names every counterpart as an ancestor, so its tree
//! outside the filtered path has to be a merge of the counterparts' outer
//! halves. Taking the first parent's counterpart tree wholesale builds the
//! lift on whatever outer state that counterpart froze, which is generally
//! the branch point rather than the integration target; the lift then reads,
//! to any later three way merge, as a deliberate revert of everything the
//! monorepo did outside the view since that branch point. Observed twice on
//! the ix monorepo: a fetch that deleted goals.md and regressed flake.lock,
//! and an integration merge of PR #10222 that regressed 19 outer paths by
//! 1255 lines.
//!
//! These tests pin the fix: the outer half of a lifted merge follows the
//! integration target's content where only one side changed it, and a path
//! both sides changed differently is an error, never a silent pick.

use std::fmt::Write as _;

use bstr::BString;
use gix::ObjectId;
use jj_views::Cache;
use jj_views::Error;
use jj_views::Filter;
use jj_views::Semantics;

const PREFIX: &str = "sub";

struct World {
    repo: gix::Repository,
    _dir: tempfile::TempDir,
}

fn world() -> World {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let repo = gix::init_bare(dir.path()).expect("an empty repository");
    World { repo, _dir: dir }
}

fn filter() -> Filter {
    Filter::prefix(PREFIX)
        .expect("a valid prefix")
        .semantics(Semantics::V2)
}

fn blob(repo: &gix::Repository, content: &str) -> ObjectId {
    repo.write_blob(content.as_bytes()).expect("a blob").detach()
}

fn tree(repo: &gix::Repository, entries: &[(&str, gix::objs::tree::EntryKind, ObjectId)]) -> ObjectId {
    let mut entries: Vec<gix::objs::tree::Entry> = entries
        .iter()
        .map(|(name, kind, oid)| gix::objs::tree::Entry {
            mode: (*kind).into(),
            filename: BString::from(*name),
            oid: *oid,
        })
        .collect();
    entries.sort();
    repo.write_object(&gix::objs::Tree { entries })
        .expect("a tree")
        .detach()
}

fn commit(repo: &gix::Repository, tree: ObjectId, parents: &[ObjectId], message: &str) -> ObjectId {
    let mut raw = format!("tree {tree}\n");
    for parent in parents {
        writeln!(raw, "parent {parent}").expect("writing to a String cannot fail");
    }
    raw.push_str(
        "author A U Thor <thor@example.invalid> 1000000000 +0000\ncommitter A U Thor \
         <thor@example.invalid> 1000000000 +0000\n\n",
    );
    raw.push_str(message);
    raw.push('\n');
    gix::objs::Write::write_buf(&repo.objects, gix::objs::Kind::Commit, raw.as_bytes())
        .expect("a commit")
}

fn entry(repo: &gix::Repository, tree: ObjectId, name: &str) -> Option<ObjectId> {
    let data = repo.find_object(tree).expect("the tree exists").detach().data;
    let decoded = gix::objs::TreeRef::from_bytes(&data, repo.object_hash()).expect("a tree");
    decoded
        .entries
        .iter()
        .find(|entry| entry.filename == name)
        .map(|entry| entry.oid.to_owned())
}

fn blob_content(repo: &gix::Repository, tree: ObjectId, name: &str) -> String {
    let oid = entry(repo, tree, name).expect("the entry exists");
    String::from_utf8(repo.find_object(oid).expect("the blob exists").detach().data)
        .expect("utf-8 content")
}

fn commit_tree(repo: &gix::Repository, commit: ObjectId) -> ObjectId {
    let raw = repo.find_object(commit).expect("the commit exists").detach().data;
    gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
        .expect("a well formed commit")
        .tree()
}

fn commit_parents(repo: &gix::Repository, commit: ObjectId) -> Vec<ObjectId> {
    let raw = repo.find_object(commit).expect("the commit exists").detach().data;
    gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
        .expect("a well formed commit")
        .parents()
        .collect()
}

/// The observed failure, reduced: outer file `outer.txt` moves on main after
/// the branch point, the view's merge commit is lifted, and the lift's outer
/// half must match main, not the branch point.
///
/// History, outer repository on the left, view on the right:
///
/// ```text
/// O1 (outer.txt=old, sub/f=1)      derives to  V0 (f=1)
/// L1 = lift of P1 onto O1          <-          P1 (f=2), child of V0
/// O2 (outer.txt=new), child of L1  elides to   P1
///                                              M = merge(V0, P1), tree f=2
/// LM = lift of M onto O2 = ?
/// ```
///
/// M is the shape GitHub writes when it merges the pull request that P1 was:
/// first parent the branch's base V0, second parent the reviewed tip P1. V0's
/// counterpart is O1, the branch point. Building LM's tree on O1 alone erases
/// the `old -> new` move of `outer.txt` that main made after the branch
/// point, which is exactly the observed goals.md deletion.
#[test]
fn a_lifted_merge_keeps_outer_changes_made_after_the_branch_point() {
    let world = world();
    let repo = &world.repo;
    let filter = filter();

    let old = blob(repo, "old\n");
    let new = blob(repo, "new\n");
    let f1 = blob(repo, "1\n");
    let f2 = blob(repo, "2\n");
    let file = gix::objs::tree::EntryKind::Blob;
    let dir = gix::objs::tree::EntryKind::Tree;

    let sub1 = tree(repo, &[("f", file, f1)]);
    let sub2 = tree(repo, &[("f", file, f2)]);
    let o1_tree = tree(repo, &[("outer.txt", file, old), (PREFIX, dir, sub1)]);
    let o1 = commit(repo, o1_tree, &[], "base");

    // The view as derivation sees it at the branch point.
    let mut cache = Cache::new();
    let v0 = jj_views::derive(repo, &o1, &filter, &mut cache)
        .expect("deriving a well formed history works")
        .expect("the prefix exists");

    // The pull request: one view commit on top of V0, and the merge GitHub
    // writes for it.
    let p1 = commit(repo, sub2, &[v0], "patch");
    let merge = commit(repo, sub2, &[v0, p1], "merge the patch");

    // The patch was lifted and integrated earlier, then main moved on with an
    // outer only change: the branch point is stale by one commit.
    let l1 = jj_views::unfilter(repo, &p1, &o1, &filter, &mut cache).expect("lifting the patch");
    let o2_tree = tree(repo, &[("outer.txt", file, new), (PREFIX, dir, sub2)]);
    let o2 = commit(repo, o2_tree, &[l1], "outer only change on main");

    // A fresh cache, as a fresh `jj views fetch` run has: everything it knows
    // comes from deriving the integration target.
    let mut cache = Cache::new();
    let derived = jj_views::derive(repo, &o2, &filter, &mut cache)
        .expect("deriving a well formed history works");
    assert_eq!(derived, Some(p1), "main's view tip is the integrated patch");

    let lifted =
        jj_views::unfilter(repo, &merge, &o2, &filter, &mut cache).expect("lifting the merge");

    let lifted_tree = commit_tree(repo, lifted);
    assert_eq!(
        blob_content(repo, lifted_tree, "outer.txt"),
        "new\n",
        "the lift's outer half must match the integration target, not the branch point"
    );
    let sub = entry(repo, lifted_tree, PREFIX).expect("the view subtree is grafted");
    assert_eq!(sub, sub2, "the view's own tree is authoritative for the prefix");
    assert_eq!(
        commit_parents(repo, lifted),
        vec![o1, o2],
        "the lift merges the branch point counterpart and the integration target"
    );
}

/// Both counterparts changed the same outer path differently: the lift must
/// surface a conflict rather than silently pick a side.
#[test]
fn a_lifted_merge_refuses_to_pick_a_side_of_an_outer_conflict() {
    let world = world();
    let repo = &world.repo;
    let filter = filter();

    let old = blob(repo, "old\n");
    let ours = blob(repo, "ours\n");
    let theirs = blob(repo, "theirs\n");
    let f1 = blob(repo, "1\n");
    let f2 = blob(repo, "2\n");
    let f3 = blob(repo, "3\n");
    let file = gix::objs::tree::EntryKind::Blob;
    let dir = gix::objs::tree::EntryKind::Tree;

    let sub1 = tree(repo, &[("f", file, f1)]);
    let sub_a = tree(repo, &[("f", file, f2)]);
    let sub_b = tree(repo, &[("f", file, f1), ("g", file, f3)]);
    let sub_merged = tree(repo, &[("f", file, f2), ("g", file, f3)]);

    let o1 = commit(
        repo,
        tree(repo, &[("outer.txt", file, old), (PREFIX, dir, sub1)]),
        &[],
        "base",
    );
    let oa = commit(
        repo,
        tree(repo, &[("outer.txt", file, ours), (PREFIX, dir, sub_a)]),
        &[o1],
        "side a",
    );
    let ob = commit(
        repo,
        tree(repo, &[("outer.txt", file, theirs), (PREFIX, dir, sub_b)]),
        &[o1],
        "side b",
    );

    let mut cache = Cache::new();
    let va = jj_views::derive(repo, &oa, &filter, &mut cache)
        .expect("deriving side a works")
        .expect("the prefix exists");
    let vb = jj_views::derive(repo, &ob, &filter, &mut cache)
        .expect("deriving side b works")
        .expect("the prefix exists");
    let merge = commit(repo, sub_merged, &[va, vb], "merge the view sides");

    let err = jj_views::unfilter(repo, &merge, &ob, &filter, &mut cache)
        .expect_err("both sides changed outer.txt, so the lift has no single answer");
    match err {
        Error::OuterConflict { path, .. } => {
            assert_eq!(path, BString::from("outer.txt"));
        }
        other => panic!("expected OuterConflict, got: {other}"),
    }
}
