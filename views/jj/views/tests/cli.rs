//! The `jj-views` command, driven the way a shell drives it.
//!
//! The roundtrip tests cover the filter. These cover the four subcommands: that
//! each writes the ref it claims to write, that `import`, `derive` and
//! `unfilter` compose into a round trip that returns the same hashes, and that
//! `verify` fails loudly rather than quietly when they do not match.
//!
//! That last one earns its keep. Every assertion `verify` makes is satisfied by
//! finding nothing, so a broken `verify` would report success loudest when it
//! had stopped checking. It is pointed at a rule known to break identity and
//! required to notice.

use std::process::Command;
use std::process::Output;

use gix::ObjectId;
use jj_views::Cache;
use jj_views::Filter;
use jj_views::Semantics;
use jj_views::fixture;

const PREFIX: &str = "vendor/upstream";
const UPSTREAM_REF: &str = "refs/heads/upstream";

/// A bare repository holding the fixture history, with a ref on its tip so the
/// command has something to name.
struct World {
    upstream: fixture::Upstream,
    /// Kept so the repository outlives the test.
    dir: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let repo = gix::init_bare(dir.path()).expect("an empty repository");
        let upstream = fixture::write_upstream(&repo).expect("the fixture history");
        repo.reference(
            UPSTREAM_REF,
            upstream.head,
            gix::refs::transaction::PreviousValue::Any,
            "the fixture tip",
        )
        .expect("a writable ref");
        Self { upstream, dir }
    }

    fn repo(&self) -> gix::Repository {
        gix::open(self.dir.path()).expect("the repository")
    }

    /// Runs the command in this repository. `-R` goes last because it is an
    /// option of each subcommand, not of the bare command.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_jj-views"))
            .args(args)
            .arg("-R")
            .arg(self.dir.path())
            .output()
            .expect("the command runs")
    }

    /// Runs the command and requires it to succeed, reporting both streams when
    /// it does not. A test that only checks the exit code makes every failure
    /// look the same.
    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "`jj-views {}` failed with {:?}\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8(output.stdout).expect("utf8 output")
    }

    fn resolve(&self, name: &str) -> ObjectId {
        let repo = self.repo();
        let mut reference = repo
            .find_reference(name)
            .expect("the ref the command wrote");
        reference.peel_to_id().expect("a commit").detach()
    }

    fn import(&self) {
        let out = self.ok(&[
            "import",
            "--path",
            PREFIX,
            "--rev",
            UPSTREAM_REF,
            "--write-ref",
            "refs/heads/injected",
        ]);
        let expected = format!("injected {} commits", self.upstream.commits.len());
        assert!(
            out.contains(&expected),
            "import must report what it injected, said: {out}"
        );
    }
}

#[test]
fn import_then_verify_returns_every_upstream_hash() {
    let world = World::new();
    world.import();

    let out = world.ok(&[
        "verify",
        "--path",
        PREFIX,
        "--rev",
        "refs/heads/injected",
        "--against",
        UPSTREAM_REF,
    ]);
    let total = world.upstream.commits.len();
    assert!(
        out.contains(&format!("{total} of {total} commits identical")),
        "verify must account for every commit, said: {out}"
    );
    assert!(
        out.contains(&format!("tip {} matches", world.upstream.head)),
        "verify must name the tip it checked, said: {out}"
    );

    world.ok(&[
        "derive",
        "--path",
        PREFIX,
        "--rev",
        "refs/heads/injected",
        "--write-ref",
        "refs/heads/view",
    ]);
    assert_eq!(
        world.resolve("refs/heads/view"),
        world.upstream.head,
        "the derived ref is upstream's own tip, not a translation of it"
    );
}

#[test]
fn verify_fails_when_the_view_is_not_the_upstream_history() {
    let world = World::new();
    world.import();

    // `unchanged-including-already-empty` is the elision rule that reads as the
    // obvious simplification and breaks hash identity. Pointing `verify` at it
    // is how we know `verify` is looking.
    let output = world.run(&[
        "verify",
        "--path",
        PREFIX,
        "--rev",
        "refs/heads/injected",
        "--against",
        UPSTREAM_REF,
        "--elide",
        "unchanged-including-already-empty",
    ]);
    assert!(
        !output.status.success(),
        "verify must fail on a view that is not the upstream history"
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(
        out.contains("does not match"),
        "verify must say which hash moved, said: {out}"
    );
}

#[test]
fn a_commit_on_the_derived_side_lifts_back_and_derives_to_itself() {
    let world = World::new();
    world.import();
    let injected = world.resolve("refs/heads/injected");

    // A commit authored against the view: parented on the view's tip, carrying
    // an earlier revision of the same subtree. Reusing a tree the fixture
    // already has keeps the test about the round trip rather than about
    // building trees.
    let repo = world.repo();
    let filter = Filter::prefix(PREFIX).expect("a valid prefix");
    let mut cache = Cache::new();
    let view_head = jj_views::derive(&repo, &injected, &filter, &mut cache)
        .expect("a derivation")
        .expect("the injected tip has a view");
    let (_, earlier) = world
        .upstream
        .commits
        .first()
        .expect("the fixture has commits");
    let earlier_tree = tree_of(&repo, earlier);
    let patch = write_commit(&repo, &earlier_tree, &view_head);

    let out = world.ok(&[
        "unfilter",
        "--path",
        PREFIX,
        "--rev",
        &patch.to_string(),
        "--onto",
        "refs/heads/injected",
        "--write-ref",
        "refs/heads/lifted",
    ]);
    assert!(
        out.contains("lifted 1 commits"),
        "unfilter must report how much it lifted, said: {out}"
    );

    // The lift continues the monorepo tip rather than landing beside it, so no
    // merge is needed to bring it back.
    let lifted = world.resolve("refs/heads/lifted");
    assert_eq!(
        parents_of(&repo, &lifted),
        vec![injected],
        "the lift belongs on the revision it was made against"
    );

    let derived = world.ok(&["derive", "--path", PREFIX, "--rev", "refs/heads/lifted"]);
    assert_eq!(
        derived.lines().next(),
        Some(patch.to_string().as_str()),
        "deriving the lift must give back the commit that was written on the derived side"
    );

    world.ok(&[
        "verify",
        "--path",
        PREFIX,
        "--rev",
        "refs/heads/lifted",
        "--against",
        &patch.to_string(),
    ]);
}

#[test]
fn the_semantics_flag_selects_which_lifting_rule_applies() {
    let world = World::new();
    world.import();
    let injected = world.resolve("refs/heads/injected");

    let repo = world.repo();
    let filter = Filter::prefix(PREFIX)
        .expect("a valid prefix")
        .semantics(Semantics::V2);
    let mut cache = Cache::new();
    let view_head = jj_views::derive(&repo, &injected, &filter, &mut cache)
        .expect("a derivation")
        .expect("the injected tip has a view");
    let (_, earlier) = world
        .upstream
        .commits
        .first()
        .expect("the fixture has commits");
    let patch = write_commit(&repo, &tree_of(&repo, earlier), &view_head);

    // The two rules only differ once a view commit has more than one
    // counterpart, so the monorepo has to move first. This commit touches
    // nothing under the prefix, so it elides and its view is still `view_head`,
    // giving that view commit a second counterpart.
    let moved = write_monorepo_commit(&repo, &injected);
    repo.reference(
        "refs/heads/monorepo",
        moved,
        gix::refs::transaction::PreviousValue::Any,
        "the monorepo moved",
    )
    .expect("a writable ref");

    let rev = patch.to_string();
    let lift = |rules: &[&str], out: &str| -> ObjectId {
        let mut args = vec![
            "unfilter",
            "--path",
            PREFIX,
            "--rev",
            rev.as_str(),
            "--onto",
            "refs/heads/monorepo",
            "--write-ref",
            out,
        ];
        args.extend_from_slice(rules);
        world.ok(&args);
        world.resolve(out)
    };

    // V1 takes the counterpart the cache learned first, which is the injected
    // commit rather than the revision named here. The default is V2, so this is
    // also the check that the flag reaches the filter at all.
    let v1 = lift(&["--semantics", "v1"], "refs/heads/lifted-v1");
    let default = lift(&[], "refs/heads/lifted-default");

    assert_ne!(
        v1, default,
        "the two rule sets lift to different commits, so the flag has to change the answer"
    );
    assert_eq!(
        parents_of(&repo, &v1),
        vec![injected],
        "V1 lands on the counterpart the cache learned first"
    );
    assert_eq!(
        parents_of(&repo, &default),
        vec![moved],
        "the default rules continue the revision named by --onto"
    );
}

/// A commit on top of `parent` that changes a file OUTSIDE the filtered prefix.
///
/// Outside matters: elision turns on the commit having changed something before
/// filtering, so a commit with an identical tree would stay in the view instead
/// of collapsing onto its parent's view commit, and the scenario would not be
/// the one under test.
fn write_monorepo_commit(repo: &gix::Repository, parent: &ObjectId) -> ObjectId {
    let blob = repo
        .write_blob(b"the monorepo moved\n")
        .expect("a writable blob")
        .detach();
    let base = tree_of(repo, parent);
    let raw = repo.find_object(base).expect("a tree").detach().data;
    let mut entries: Vec<gix::objs::tree::Entry> =
        gix::objs::TreeRef::from_bytes(&raw, repo.object_hash())
            .expect("a well formed tree")
            .entries
            .iter()
            .map(|entry| gix::objs::tree::Entry {
                mode: entry.mode,
                filename: entry.filename.to_owned(),
                oid: entry.oid.to_owned(),
            })
            .collect();
    entries.push(gix::objs::tree::Entry {
        mode: gix::objs::tree::EntryKind::Blob.into(),
        filename: "MONOREPO".into(),
        oid: blob,
    });
    entries.sort();
    let tree = repo
        .write_object(&gix::objs::Tree { entries })
        .expect("a writable tree")
        .detach();
    let raw = format!(
        "tree {tree}\nparent {parent}\nauthor Mono <mono@example.invalid> 1700000001 \
         +0000\ncommitter Mono <mono@example.invalid> 1700000001 +0000\n\nthe monorepo moved\n"
    );
    gix::objs::Write::write_buf(&repo.objects, gix::objs::Kind::Commit, raw.as_bytes())
        .expect("a writable object")
}

fn tree_of(repo: &gix::Repository, commit: &ObjectId) -> ObjectId {
    let raw = repo.find_object(*commit).expect("a commit").detach().data;
    gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
        .expect("a well formed commit")
        .tree()
}

fn parents_of(repo: &gix::Repository, commit: &ObjectId) -> Vec<ObjectId> {
    let raw = repo.find_object(*commit).expect("a commit").detach().data;
    gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
        .expect("a well formed commit")
        .parents()
        .collect()
}

/// A commit with fixed metadata, so the test's object ids do not move between
/// runs.
fn write_commit(repo: &gix::Repository, tree: &ObjectId, parent: &ObjectId) -> ObjectId {
    let raw = format!(
        "tree {tree}\nparent {parent}\nauthor Local <local@example.invalid> 1700000000 \
         +0000\ncommitter Local <local@example.invalid> 1700000000 +0000\n\na local patch\n"
    );
    gix::objs::Write::write_buf(&repo.objects, gix::objs::Kind::Commit, raw.as_bytes())
        .expect("a writable object")
}
