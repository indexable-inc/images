//! Whether a commit the view drops is still known to have arrived.
//!
//! The view's elision rule keeps a history clean and keeps its hashes, and it
//! costs one thing: a commit it drops leaves no trace, so `derive` answers "the
//! view does not contain this" for a commit that is right here. Anything
//! deciding what a published repository has that this one does not reads that
//! as "not fetched yet", lifts the commit, elides it again, and never
//! converges. ENG-11873 was three copies of one upstream merge, one per `jj
//! views fetch` against a remote that had not moved.
//!
//! `Cache::elided` and `verify::integrated` are the answer, and this pins them
//! against the shape that produced the bug.

use std::fmt::Write as _;

use gix::ObjectId;
use jj_views::Cache;
use jj_views::Elide;
use jj_views::Filter;
use jj_views::Semantics;

const PREFIX: &str = "ix";

fn filter() -> Filter {
    Filter::prefix(PREFIX)
        .expect("a usable prefix")
        .semantics(Semantics::V2)
        .elide(Elide::Unchanged)
}

fn blob(repo: &gix::Repository, content: &str) -> ObjectId {
    repo.write_blob(content.as_bytes())
        .expect("a blob")
        .detach()
}

/// A tree of blobs, or of one subtree named by `PREFIX` plus blobs.
fn tree(repo: &gix::Repository, entries: &[(&str, ObjectId, bool)]) -> ObjectId {
    let mut tree = gix::objs::Tree::default();
    for (name, id, is_tree) in entries {
        tree.entries.push(gix::objs::tree::Entry {
            mode: if *is_tree {
                gix::objs::tree::EntryKind::Tree.into()
            } else {
                gix::objs::tree::EntryKind::Blob.into()
            },
            filename: (*name).into(),
            oid: *id,
        });
    }
    tree.entries.sort();
    repo.write_object(&tree).expect("a tree").detach()
}

fn commit(
    repo: &gix::Repository,
    tree: ObjectId,
    parents: &[ObjectId],
    message: &str,
    when: u32,
) -> ObjectId {
    let mut raw = format!("tree {tree}\n");
    for parent in parents {
        writeln!(raw, "parent {parent}").expect("a growable string");
    }
    write!(
        raw,
        "author A <a@example.invalid> {when}00 +0000\ncommitter A <a@example.invalid> {when}00 \
         +0000\n\n{message}\n"
    )
    .expect("a growable string");
    gix::objs::Write::write_buf(&repo.objects, gix::objs::Kind::Commit, raw.as_bytes())
        .expect("a writable object database")
}

/// The state ENG-11873 was found in, built from raw objects.
///
/// A published repository whose tip is a merge of a branch that had already
/// landed, and a monorepo holding the published history under `PREFIX` plus a
/// commit of its own outside it. That last commit is not decoration: the merge
/// is dropped only because its two parents' counterparts here disagree outside
/// the prefix, which is what "changed nothing under the prefix, changed
/// something outside it" means.
struct World {
    repo: gix::Repository,
    /// The published tip: a merge of `landed` whose tree equals both parents'.
    published: ObjectId,
    /// The monorepo tip, holding the published history up to `merge`'s parents.
    monorepo: ObjectId,
    _dir: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let repo = gix::init_bare(dir.path()).expect("a bare repository");

        // The published repository: two commits, then a branch forked from the
        // first that arrives at the second's tree by another route -- a change
        // cherry-picked onto main and the branch merged anyway -- and a merge
        // of it whose tree equals both parents'.
        let one = blob(&repo, "one\n");
        let two = blob(&repo, "two\n");
        let first_tree = tree(&repo, &[("a", one, false)]);
        let second_tree = tree(&repo, &[("a", one, false), ("b", two, false)]);
        let first = commit(&repo, first_tree, &[], "first", 1);
        let second = commit(&repo, second_tree, &[first], "second", 2);
        let landed = commit(
            &repo,
            second_tree,
            &[first],
            "the same change, on a branch",
            3,
        );
        let published = commit(
            &repo,
            second_tree,
            &[second, landed],
            "merge a branch that had already landed",
            4,
        );

        // The monorepo: the published history injected under the prefix, then a
        // commit of its own that touches nothing under it.
        let filter = filter();
        let mut cache = Cache::new();
        let outside = blob(&repo, "monorepo\n");
        let base = commit(
            &repo,
            tree(&repo, &[("outside", outside, false)]),
            &[],
            "monorepo base",
            10,
        );
        let mut monorepo = base;
        for source in [first, second] {
            monorepo = jj_views::unfilter(&repo, &source, &monorepo, &filter, &mut cache)
                .expect("the commit lifts");
        }
        let changed = blob(&repo, "monorepo, later\n");
        let subtree = jj_views::derive(&repo, &monorepo, &filter, &mut cache)
            .expect("a derivation")
            .expect("a view tip");
        let subtree = commit_tree(&repo, &subtree);
        let monorepo = commit(
            &repo,
            tree(
                &repo,
                &[("outside", changed, false), (PREFIX, subtree, true)],
            ),
            &[monorepo],
            "a monorepo change outside the prefix",
            11,
        );

        Self {
            repo,
            published,
            monorepo,
            _dir: dir,
        }
    }

    /// One round of what `jj views fetch` does: work out what is missing, lift
    /// it, and report the new monorepo tip and how many commits were lifted.
    fn fetch(&self, from: ObjectId) -> (ObjectId, usize) {
        // A fresh cache each round, because each `jj views fetch` is a process.
        let mut cache = Cache::new();
        let filter = filter();
        let integrated = jj_views::verify::integrated(&self.repo, &from, &filter, &mut cache)
            .expect("a derivation");
        let incoming: Vec<ObjectId> = jj_views::verify::ancestry(&self.repo, &self.published)
            .expect("the published ancestry")
            .into_iter()
            .filter(|id| !integrated.contains(id))
            .collect();
        let mut head = from;
        for source in &incoming {
            let lifted = jj_views::unfilter(&self.repo, source, &from, &filter, &mut cache)
                .expect("the commit lifts");
            if *source == self.published {
                head = lifted;
            }
        }
        (head, incoming.len())
    }
}

fn commit_tree(repo: &gix::Repository, commit: &ObjectId) -> ObjectId {
    let raw = repo.find_object(*commit).expect("the commit").detach().data;
    gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
        .expect("a commit")
        .tree()
}

/// The bug: the second fetch of a remote that has not moved lifts again.
#[test]
fn a_fetched_view_converges_even_when_the_published_tip_is_elided() {
    let world = World::new();

    let (first, lifted) = world.fetch(world.monorepo);
    assert_eq!(lifted, 2, "the branch and the merge should both be lifted");
    assert_ne!(first, world.monorepo, "the first fetch lifted nothing");

    // The shape is only interesting if the view really does drop the merge. If
    // a future change to the elision rule stops dropping it, this test would
    // pass for the wrong reason and stop guarding anything.
    let mut cache = Cache::new();
    let derived = jj_views::derive(&world.repo, &first, &filter(), &mut cache)
        .expect("a derivation")
        .expect("a view tip");
    assert_ne!(
        derived, world.published,
        "the view no longer elides the published tip, so this test is not exercising ENG-11873 \
         any more and needs a new shape"
    );

    let (second, lifted) = world.fetch(first);
    assert_eq!(
        lifted, 0,
        "the second fetch of an unmoved remote lifted {lifted} commits again"
    );
    assert_eq!(second, first, "the second fetch moved the view");
}

/// The mechanism underneath, named directly so a failure says which half broke.
#[test]
fn an_elided_commit_records_the_view_commit_it_would_have_been() {
    let world = World::new();
    let (lifted, _) = world.fetch(world.monorepo);

    let mut cache = Cache::new();
    let integrated = jj_views::verify::integrated(&world.repo, &lifted, &filter(), &mut cache)
        .expect("a derivation");
    assert!(
        !integrated.derived.contains(&world.published),
        "the view was expected to elide the published tip"
    );
    assert!(
        integrated.elided.contains(&world.published),
        "the elided published tip was not recorded, so it reads as never fetched"
    );
    assert!(integrated.contains(&world.published));
}
