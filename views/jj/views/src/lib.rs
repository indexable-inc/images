//! Deterministic path filters over git history.
//!
//! A *view* is a child repository that is nothing but a pure function of a
//! parent monorepo's history. [`derive()`] computes it: given a parent commit
//! and a path, it produces the commit of the history restricted to that path.
//! [`unfilter`] is the inverse, lifting a commit of the view back into the
//! parent.
//!
//! The property the rest of the design rests on is *round trip hash identity*.
//! Take an upstream repository, inject all of its history into a parent repo by
//! moving every tree under `vendor/upstream/` and copying commit metadata
//! verbatim, then [`derive()`] the parent with the filter `vendor/upstream`.
//! The commits that come back out carry upstream's original hashes, byte for
//! byte. That is what makes the view share real ancestry with upstream, so
//! merge bases are correct and syncing is an ordinary fetch and rebase rather
//! than a translation layer.
//!
//! Identity survives elision, but only because the elision rule asks whether a
//! commit was empty *before* filtering rather than only after. See [`Elide`]
//! for why that one condition is what makes a view both clean and hash
//! compatible.
//!
//! Only prefix filters are supported.
//!
//! # Where the rules came from
//!
//! The filter policy here is josh's, reimplemented rather than invented, and a
//! reader who finds a rule arbitrary should go read josh before changing it.
//! The rules were taken from these files at josh `r26.07.28`:
//!
//! - `josh-core/src/history.rs`. `create_filtered_commit2` for the order the
//!   decisions are made in, `select_parent_commits` for the elision rule and
//!   its `all_diffs_empty` guard, the block above it for the trivial merge rule
//!   and its `was_trivial_merge` guard, and `rewrite_commit` for rewriting a
//!   commit through `gix_object::CommitRef` so author and committer bytes
//!   survive.
//! - `josh-core/src/filter/opt.rs` and its `invert` for the inverse algebra.
//!
//! Two deliberate divergences, both recorded in [`Semantics`]: josh does not
//! deduplicate filtered parents and so can emit a merge whose two parents are
//! the same commit, and this crate rewrites only the `tree` and `parent` lines
//! at the byte level rather than reserializing a parsed commit.
//!
//! What is not reimplemented is josh's cram suite: 181 `.t` files, of which the
//! 79 under `tests/filter` are the ones about filter semantics rather than the
//! proxy or the CLI. It is the most valuable thing in that repository and the
//! part hardest to rebuild, and it should be read as a checklist of cases this
//! crate has not thought about even though the harness cannot be reused. There
//! are 19 tests here against those 79, which is the honest measure of how much
//! behavior is untested rather than wrong.

#![deny(missing_docs)]

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use bstr::BString;
use bstr::ByteSlice as _;
use gix::ObjectId;
use gix::hash::oid;
use gix::objs::Write as _;

mod raw;

pub mod fixture;
pub mod verify;

/// Anything that can go wrong deriving or unfiltering a view.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A spec file could not be read or written.
    #[error("could not read or write the filter spec at {path}")]
    SpecIo {
        /// The file in question.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A spec recorded a semantics version this build does not implement.
    ///
    /// Refusing is the point. Deriving under different rules than the ones that
    /// produced a published history moves every hash after the first
    /// difference, and a history that has been published cannot be
    /// un-published.
    #[error(
        "filter spec records semantics version {recorded:?}, which this build does not implement; \
         it knows {known}"
    )]
    UnknownSemantics {
        /// The version the repository recorded.
        recorded: String,
        /// The versions this build can apply, comma separated.
        known: String,
    },
    /// A filter spec was missing a field, had an unknown field, or named a
    /// value this version does not know.
    #[error("filter spec {spec:?} is not one this version can read")]
    BadSpec {
        /// The rejected spec.
        spec: String,
    },
    /// The filter path was not usable as a prefix.
    #[error("filter path {path:?} must be relative, with no empty or dot components")]
    BadFilterPath {
        /// The rejected path.
        path: String,
    },
    /// An object could not be read from the repository.
    #[error("could not read object {id}")]
    Find {
        /// The object that could not be read.
        id: ObjectId,
        /// The underlying object database error.
        #[source]
        source: Box<gix::object::find::existing::Error>,
    },
    /// An object could not be written.
    #[error("could not write object")]
    Write(#[from] gix::objs::write::Error),
    /// An object's id could not be computed.
    #[error("could not compute an object id")]
    Hash(#[from] gix::hash::hasher::Error),
    /// A commit object did not begin with a `tree` line followed by its
    /// `parent` lines.
    #[error("commit object is malformed")]
    MalformedCommit,
    /// A commit-graph ancestry walk could not be initialized.
    #[error("could not initialize revision walk")]
    RevisionWalk(#[source] Box<gix::revision::walk::Error>),
    /// A commit-graph ancestry walk could not read one of its commits.
    #[error("could not walk revision history")]
    RevisionWalkCommit(#[source] Box<gix::revision::walk::iter::Error>),
    /// An object was not the kind its referrer claimed.
    #[error("expected object {id} to be a {expected}")]
    WrongKind {
        /// The object in question.
        id: ObjectId,
        /// The kind that was expected.
        expected: gix::objs::Kind,
    },
    /// A commit of the view could not be lifted back into the parent, because
    /// one of its parents has no known position there.
    #[error("commit {id} of the view has no counterpart in the parent repository")]
    Ungrafted {
        /// The view commit with no counterpart.
        id: ObjectId,
    },
    /// A derivation anchor names a source commit outside the requested history.
    #[error("view anchor source {anchor_source} is not an ancestor of {revision}")]
    AnchorNotAncestor {
        /// The source-side anchor.
        anchor_source: ObjectId,
        /// The revision being derived.
        revision: ObjectId,
    },
    /// The two sides of an anchor do not describe the same filtered snapshot.
    #[error(
        "view anchor {anchor_source} -> {view} has different trees: filtered source \
         {source_tree}, view {view_tree}"
    )]
    AnchorTreeMismatch {
        /// The source-side anchor.
        anchor_source: ObjectId,
        /// The published view anchor.
        view: ObjectId,
        /// The source tree after filtering.
        source_tree: Box<ObjectId>,
        /// The published view's tree.
        view_tree: ObjectId,
    },
    /// A fetched anchor object did not hash to the manifest's published id.
    #[error("fetched view anchor hashes to {actual}, expected {expected}")]
    AnchorObjectMismatch {
        /// Object id recorded by the manifest.
        expected: ObjectId,
        /// Object id computed from the fetched bytes.
        actual: ObjectId,
    },
    /// The history walk finding a merge base between two counterpart parents
    /// of a lifted merge failed.
    #[error("could not find a merge base between {first} and {second}")]
    MergeBase {
        /// The first counterpart commit.
        first: ObjectId,
        /// The second counterpart commit.
        second: ObjectId,
        /// The underlying error.
        #[source]
        source: Box<gix::repository::merge_base::Error>,
    },
    /// Two parents of a lifted merge changed the same path outside the view
    /// differently.
    ///
    /// The lifted commit names both counterparts as ancestors, so its tree
    /// outside the view has to merge their outer halves. A path both sides
    /// changed since their merge base has no single answer, and picking one
    /// silently would hand the loser's content to every later merge as a
    /// deliberate revert. Integrate the counterparts in the parent repository
    /// first, then lift again.
    #[error(
        "lifting the merge would pick one side of {path:?}: {ours} and {theirs} both changed it"
    )]
    OuterConflict {
        /// The conflicting path outside the view, slash separated.
        path: BString,
        /// The counterpart on the first parent's side.
        ours: ObjectId,
        /// The counterpart whose outer half was being merged in.
        theirs: ObjectId,
    },
}

/// A checked starting point for an incremental view derivation.
///
/// The pair is only trusted after [`Cache::seed_anchor`] proves that `source`
/// is in the requested history and that filtering its tree yields `view`'s
/// tree. A commit pair in a manifest is therefore a claim to check, never a
/// cache entry to accept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeriveAnchor {
    /// Commit in the repository that owns the view path.
    pub source: ObjectId,
    /// Published view commit for the same snapshot.
    pub view: ObjectId,
}

/// Computes the parentless view anchor for one source snapshot.
///
/// The id is suitable for `views.NAME.anchor.view` when that anchor also sets
/// `root = true`. The commit keeps the source commit's metadata and message,
/// replaces its tree with the filtered tree, and removes every parent.
pub fn root_anchor_id(
    repo: &gix::Repository,
    source: &oid,
    filter: &Filter,
) -> Result<ObjectId, Error> {
    let (_, id, _) = root_anchor(repo, source, filter)?;
    Ok(id)
}

/// A path prefix to restrict history to.
///
/// Two filters differing only in their [`Elide`] policy or their trivial merge
/// policy are distinct filters and get distinct cache entries, because they
/// produce different commits.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Filter {
    components: Vec<BString>,
    elide: Elide,
    keep_trivial_merges: bool,
    semantics: Semantics,
}

/// The conventional file recording which rules produced a repository's views.
///
/// At the root of the repository holding the derived history, so that a
/// consumer can answer "which semantics is this history under" from the
/// repository alone.
pub const SPEC_FILE_NAME: &str = ".jj-views";

/// The version of the hash affecting rules a filter follows.
///
/// Every decision that can change an output commit hash is pinned by this
/// number, and the number is written into the repository next to the filter so
/// a view records what produced it. Without that, a change to any rule silently
/// produces a different history from the same input, and the only symptom is
/// that hashes stop matching a view somebody already has.
///
/// This is not hypothetical. josh has broken its own output hashes at least
/// twice and its compatibility flags are the fossils: `gpgsig="norm-lf"` exists
/// only to reproduce histories from a josh that normalized CRLF inside
/// `gpgsig`, and `history="keep-trivial-merges"` was the default before it
/// became opt in. rust-lang generated commits with a josh that stripped
/// signatures and had to force push the entire history of rustc-dev-guide to
/// recover, and their README now pins an exact josh tag with the reason written
/// down.
///
/// Adding a rule, or changing one, means adding a variant here. Old variants
/// keep their old behavior forever; that is the entire point of them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Semantics {
    /// The initial rules.
    ///
    /// Enumerated from the code rather than from memory, because a rule nobody
    /// thought to version is indistinguishable from one that cannot change.
    /// Each line names where it lives.
    ///
    /// **What is rewritten.**
    /// 1. Only the `tree` and `parent` lines are replaced; every other byte of
    ///    the commit object is copied through, including `author`, `committer`,
    ///    `encoding`, `gpgsig`, `mergetag`, unknown extra headers, their
    ///    relative order, and the message (`raw::replace_ids`).
    /// 2. Object ids are written as lowercase hex at the repository's full hash
    ///    length (`raw::write_id_line`).
    ///
    /// **Parents.**
    /// 3. Parents keep the source commit's order
    ///    (`Derivation::derived_parents`).
    /// 4. A duplicate the source commit already had is kept; a duplicate the
    ///    filter introduced by collapsing two distinct parents onto one view
    ///    commit is dropped (`Derivation::derived_parents`).
    ///
    /// **What is dropped.**
    /// 5. Elision follows the filter's [`Elide`] policy, which turns on whether
    ///    the commit was empty *before* filtering (`Derivation::map`).
    /// 6. Trivial merges follow the filter's trivial merge policy, and a merge
    ///    already trivial before filtering is never dropped
    ///    (`Derivation::trivial_merge_target`).
    ///
    /// **Trees.**
    /// 7. A path absent from a commit's tree filters to the empty tree, so a
    ///    commit deleting the filtered directory appears in the view as a
    ///    commit that empties it (`Derivation::map`).
    /// 8. A path component that names a blob rather than a tree counts as
    ///    absent when filtering, and is replaced when grafting (`lookup`,
    ///    `graft_tree`).
    /// 9. Tree entries are ordered by git's rule, with a directory name
    ///    compared as though it ended in a slash (`graft_tree`, via
    ///    `gix_object::tree::Entry`'s `Ord`).
    /// 10. Intermediate directories created by a graft get mode `40000`
    ///     (`graft_tree`).
    ///
    /// **Lifting.**
    /// 11. A lifted commit's tree is built on its parents' counterparts, the
    ///     outer halves of several being merged three way, and on `onto` only
    ///     when no parent has one (`unfilter`).
    /// 12. A lifted commit's parents are its parents' counterparts, with a
    ///     single unknown parent landing on `onto` and an unknown parent of a
    ///     merge being refused (`unfilter`).
    /// 13. Where a view commit has SEVERAL parent-repo counterparts, which it
    ///     does whenever a monorepo commit elided onto an existing view commit,
    ///     the one used is whichever the cache learned first. That is an
    ///     artifact rather than a rule: it depends on the order the caller
    ///     derived things in, so V1 lifting is reproducible only against an
    ///     identical sequence of calls. [`Semantics::V2`] specifies it instead.
    ///
    /// **Inputs to all of the above.**
    /// 13. The prefix is normalized by trimming outer slashes, and rejects
    ///     empty, dot, and `;`-bearing components (`Filter::prefix`).
    /// 14. The repository's hash kind is used throughout, so the same rules
    ///     over a SHA-256 repository produce SHA-256 ids. Untested there.
    ///
    /// Rule 1 has a useful consequence: a commit message trailer convention for
    /// per-commit upstream provenance, of the `UPSTREAM:` or `Git-commit:` kind
    /// that kernel forks converge on, rides through derivation and lifting in
    /// both directions untouched. Adopting one is a decision about how
    /// commits are written, not about this filter, and needs no version
    /// bump here.
    #[default]
    V1,
    /// [`V1`](Self::V1) with one lifting rule specified rather than incidental.
    ///
    /// Rule 13 of V1 leaves the choice among a view commit's several
    /// parent-repo counterparts to cache insertion order. V2 replaces it:
    ///
    /// 13. Where `onto` is itself a counterpart of a lifted commit's parent,
    ///     `onto` is the one used. Otherwise the cache's answer stands
    ///     (`unfilter`).
    ///
    /// Everything else is V1 unchanged, including all of derivation, so a view
    /// derived under either version is the same view. Only where a lifted
    /// commit lands differs.
    ///
    /// The reason to specify it is that the incidental answer is the wrong one
    /// in the case the design is for. After an import, the earliest counterpart
    /// of the view's tip is the injected commit inside the vendored lineage,
    /// while the monorepo tip is a later one. Lifting onto the injected commit
    /// puts the result beside the monorepo instead of on it, so a merge is
    /// needed to bring it back, and that merge reverse-maps into a merge commit
    /// in the view that whoever wrote the change never wrote. One per round
    /// trip, accumulating.
    ///
    /// Under V2 the lift is a fast-forward on the monorepo tip and re-deriving
    /// returns the commit byte for byte, so a round trip adds nothing.
    V2,
}

impl Semantics {
    /// Every version this build can apply, as they appear in a spec.
    pub const KNOWN: &'static [&'static str] = &["1", "2"];

    /// The number as it appears in a filter spec.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V1 => "1",
            Self::V2 => "2",
        }
    }
}

/// What to do with a commit whose filtered tree is unchanged from its parent's.
///
/// This is hash affecting, so it is part of the filter's identity and belongs
/// in a recorded semantics version rather than in a caller's discretion.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Elide {
    /// Keep every commit. The view has one commit per parent commit.
    Nothing,
    /// Drop it, unless the commit was already empty *before* filtering.
    ///
    /// The exception is the whole trick, and it is what lets a view be both
    /// clean and hash compatible. A monorepo commit that touched only some
    /// other directory has a tree that differs from its parent's, so it is
    /// dropped and does not clutter the view. An upstream commit that was
    /// deliberately empty has a tree identical to its parent's before
    /// filtering as well as after, so it survives, and the hashes of it and
    /// everything after it are preserved.
    ///
    /// This matches josh's default, implemented in `select_parent_commits` in
    /// `josh-core/src/history.rs` as `if affects_filtered || all_diffs_empty`.
    Unchanged,
    /// Drop it whether or not it was already empty before filtering.
    ///
    /// Do not use this on a history whose hashes must match an upstream. It is
    /// here because it is the rule one writes by accident, it looks correct,
    /// and the damage is invisible until something compares hashes: on
    /// git.git's 85050 commits it drops 7 deliberately empty commits and,
    /// because a moved hash changes every descendant's parent line, moves
    /// 78500 more.
    UnchangedIncludingAlreadyEmpty,
}

impl Filter {
    /// A filter keeping only what lives under `path`.
    ///
    /// Defaults to [`Elide::Unchanged`] and to dropping trivial merges, which
    /// is what josh does by default and what preserves hash identity.
    pub fn prefix(path: &str) -> Result<Self, Error> {
        let components: Vec<BString> = path
            .trim_matches('/')
            .split('/')
            .map(|part| BString::from(part.as_bytes()))
            .collect();
        // An empty component comes from a doubled slash, and a dot component
        // would make it ambiguous which tree entry the filter names. A `;` or a
        // control byte is refused so that a filter spec, which separates its
        // fields with `;`, can be parsed back without an escaping scheme. git
        // permits both in a path; no vendoring mount point needs them.
        let usable = components.iter().all(|part| {
            !matches!(part.as_slice(), b"" | b"." | b"..")
                && !part
                    .iter()
                    .any(|byte| *byte == b';' || byte.is_ascii_control())
        });
        if !usable {
            return Err(Error::BadFilterPath {
                path: path.to_owned(),
            });
        }
        Ok(Self {
            components,
            elide: Elide::Unchanged,
            keep_trivial_merges: false,
            semantics: Semantics::default(),
        })
    }

    /// Sets what happens to a commit whose filtered tree is unchanged.
    #[must_use]
    pub fn elide(mut self, elide: Elide) -> Self {
        self.elide = elide;
        self
    }

    /// Whether a merge whose filtered tree equals its first filtered parent's
    /// is kept.
    ///
    /// Dropping them, the default, is what keeps a view from filling up with
    /// merges that say nothing about the filtered path; rust-lang called the
    /// merge flood the main problem they hit with josh, over 10000 merges for
    /// one initial sync. Keeping them preserves the branch structure at the
    /// cost of degenerate merges whose parents collapse onto one chain.
    ///
    /// A merge that was *already* trivial before filtering is never dropped,
    /// for the same reason an already empty commit is not: dropping it would
    /// move its hash and every descendant's.
    #[must_use]
    pub fn keep_trivial_merges(mut self, keep: bool) -> Self {
        self.keep_trivial_merges = keep;
        self
    }

    /// Sets the semantics version.
    ///
    /// Only needed to reproduce a view built by an older version of this crate.
    #[must_use]
    pub fn semantics(mut self, semantics: Semantics) -> Self {
        self.semantics = semantics;
        self
    }

    /// The canonical spec string, which is what gets written into a repository
    /// beside the view so it records the rules that produced it.
    ///
    /// Round trips through [`parse`](Self::parse).
    #[must_use]
    pub fn spec(&self) -> String {
        let elide = match self.elide {
            Elide::Nothing => "nothing",
            Elide::Unchanged => "unchanged",
            Elide::UnchangedIncludingAlreadyEmpty => "including-already-empty",
        };
        let merges = if self.keep_trivial_merges {
            "keep"
        } else {
            "drop"
        };
        format!(
            "semantics={};prefix={};elide={elide};trivial-merges={merges}",
            self.semantics.as_str(),
            self.path()
        )
    }

    /// Reads a filter back from its [`spec`](Self::spec).
    ///
    /// Every field is required and unknown fields are refused, so a spec
    /// written by a newer version fails loudly here rather than being read
    /// as though the missing rules did not exist.
    pub fn parse(spec: &str) -> Result<Self, Error> {
        let bad = || Error::BadSpec {
            spec: spec.to_owned(),
        };
        let mut semantics = None;
        let mut prefix = None;
        let mut elide = None;
        let mut merges = None;
        for field in spec.split(';') {
            let (key, value) = field.split_once('=').ok_or_else(bad)?;
            match key {
                "semantics" => {
                    semantics = Some(match value {
                        "1" => Semantics::V1,
                        "2" => Semantics::V2,
                        // An older version this build still implements is applied
                        // as recorded; that is what old variants are for. Only a
                        // version this build does not know is refused, and it is
                        // refused by name so the operator can see what to install.
                        other => {
                            return Err(Error::UnknownSemantics {
                                recorded: other.to_owned(),
                                known: Semantics::KNOWN.join(", "),
                            });
                        }
                    });
                }
                "prefix" => prefix = Some(value),
                "elide" => {
                    elide = Some(match value {
                        "nothing" => Elide::Nothing,
                        "unchanged" => Elide::Unchanged,
                        "including-already-empty" => Elide::UnchangedIncludingAlreadyEmpty,
                        _ => return Err(bad()),
                    });
                }
                "trivial-merges" => {
                    merges = Some(match value {
                        "keep" => true,
                        "drop" => false,
                        _ => return Err(bad()),
                    });
                }
                _ => return Err(bad()),
            }
        }
        Ok(Self::prefix(prefix.ok_or_else(bad)?)?
            .semantics(semantics.ok_or_else(bad)?)
            .elide(elide.ok_or_else(bad)?)
            .keep_trivial_merges(merges.ok_or_else(bad)?))
    }

    /// Writes this filter's spec to `path`, creating parent directories.
    ///
    /// The file is the record a consumer reads to learn which rules produced
    /// the history they are holding, without knowing which binary built it.
    /// A pinned revision of a build tool is not a substitute: it says which
    /// code ran, not which rules that code applied, and it is not readable
    /// from the repository that carries the history.
    ///
    /// Conventionally [`SPEC_FILE_NAME`] at the root of the repository holding
    /// the view. The format is one spec per line, so a repository with
    /// several views records several lines, and a leading `#` comment line
    /// is ignored.
    pub fn write_spec(&self, path: &Path) -> Result<(), Error> {
        let write = |body: String| -> Result<(), Error> {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).map_err(|source| Error::SpecIo {
                    path: path.to_path_buf(),
                    source,
                })?;
            }
            std::fs::write(path, body).map_err(|source| Error::SpecIo {
                path: path.to_path_buf(),
                source,
            })
        };
        write(format!(
            "# Rules that produced the derived views in this repository. Do not\n# edit: a \
             history published under one set of rules cannot be\n# republished under another \
             without moving every hash.\n{}\n",
            self.spec()
        ))
    }

    /// Reads the filters recorded at `path`.
    ///
    /// Comment and blank lines are skipped. A line this version cannot fully
    /// read is an error rather than a filter with defaults filled in.
    pub fn read_specs(path: &Path) -> Result<Vec<Self>, Error> {
        let body = std::fs::read_to_string(path).map_err(|source| Error::SpecIo {
            path: path.to_path_buf(),
            source,
        })?;
        body.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(Self::parse)
            .collect()
    }

    /// The path this filter keeps, as `a/b/c`.
    #[must_use]
    pub fn path(&self) -> BString {
        let mut out = BString::default();
        for (at, component) in self.components.iter().enumerate() {
            if at > 0 {
                out.push(b'/');
            }
            out.extend_from_slice(component);
        }
        out
    }
}

/// Memoized results, safe to reuse across calls and across filters.
///
/// The cache is keyed on `(tree, filter)` rather than on commits, because that
/// is where the sharing is: a monorepo commit that did not touch the filtered
/// path has the same subtree as its parent, and in a real history the large
/// majority of commits are in that position. Reusing a cache is what keeps an
/// incremental derivation proportional to what changed rather than to the size
/// of history.
///
/// Measured, because that claim is worth nothing unmeasured.
///
/// All 85050 commits of git.git under a two component prefix: about 45 seconds
/// to inject and 45 to derive. All 1464098 commits of linux: 911s to inject,
/// 1024s to derive, 3.9 GB peak resident set, and every hash returned. Adding
/// ten commits on top and re-deriving with the same cache takes 0.0007 seconds
/// and exactly ten tree reads, one per commit for the one tree that changed.
///
/// The first two are one time import costs. The last is what a fetch costs, and
/// it is the one that decides whether a view is usable.
///
/// Two things about the import cost are worth knowing before anyone optimizes
/// it. It is bound by writing one loose object per tree per commit rather than
/// by this code: the kernel run spent 157 seconds of user time inside 1949
/// seconds of wall clock, with system time 6.7 times user time. And it needs no
/// blobs at all. That measurement ran against a `--filter=blob:none` clone, so
/// deriving and lifting read commits and trees only, and a deployment
/// maintaining a view never has to fetch file contents.
#[derive(Default)]
pub struct Cache {
    per_filter: HashMap<Filter, FilterCache>,
    reachable: HashMap<ObjectId, std::collections::HashSet<ObjectId>>,
    ancestry_walks: usize,
    exhaustive_derivations: usize,
}

#[derive(Default)]
struct FilterCache {
    /// Subtree lookups, keyed on the tree and how many path components have
    /// been consumed so far, `None` when the path is absent below it.
    ///
    /// Keying on the root tree alone gives almost no sharing, which is worth
    /// spelling out because it is the opposite of what one expects: a monorepo
    /// commit that touched some other directory has a *different* root tree and
    /// the *same* filtered subtree, and that is the common case. The sharing is
    /// one level down, so every level is memoized and a commit that left the
    /// filtered path alone is answered from the first shared tree on the way
    /// in.
    trees: HashMap<(ObjectId, usize), Option<ObjectId>>,
    /// Parent commit to view commit, `None` when nothing of the path exists in
    /// its ancestry.
    commits: HashMap<ObjectId, Option<ObjectId>>,
    /// View commit to its own tree, so elision does not have to re-read the
    /// parent's commit object once per commit.
    view_trees: HashMap<ObjectId, ObjectId>,
    /// Parent-repo commit to its own unfiltered tree. Needed because the
    /// elision rule turns on whether a commit was empty *before* filtering, so
    /// the unfiltered trees of its parents have to be on hand.
    source_trees: HashMap<ObjectId, ObjectId>,
    /// View commit back to the parent commit it came from, for [`unfilter`].
    grafts: HashMap<ObjectId, ObjectId>,
    /// Parent commit to the view commit it *would* have been, for a commit the
    /// view drops. See [`Derivation::record_elided`].
    elided: HashMap<ObjectId, ObjectId>,
    /// Tree objects read while descending, so the cost of a derivation can be
    /// measured rather than assumed.
    tree_reads: usize,
    /// Commit objects read while traversing source history.
    commit_reads: usize,
}

impl Cache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many `(tree, filter)` pairs are memoized, over all filters.
    #[must_use]
    pub fn tree_entries(&self) -> usize {
        self.per_filter.values().map(|per| per.trees.len()).sum()
    }

    /// How many commits are memoized, over all filters.
    #[must_use]
    pub fn commit_entries(&self) -> usize {
        self.per_filter.values().map(|per| per.commits.len()).sum()
    }

    /// How many tree objects have been read, over all filters.
    ///
    /// This is the number the cache exists to hold down. A commit that did not
    /// touch the filtered path should cost one read, not one per path
    /// component, because every tree on the way in except the changed root
    /// is shared with its parent commit.
    #[must_use]
    pub fn tree_reads(&self) -> usize {
        self.per_filter.values().map(|per| per.tree_reads).sum()
    }

    /// How many source commit objects derivation has read, over all filters.
    #[must_use]
    pub fn commit_reads(&self) -> usize {
        self.per_filter.values().map(|per| per.commit_reads).sum()
    }

    /// Discards derived state for `filter` while retaining shared ancestry
    /// checks.
    pub fn discard_filter(&mut self, filter: &Filter) {
        self.per_filter.remove(filter);
    }

    /// How many host revision graphs anchor validation has traversed.
    #[must_use]
    pub fn ancestry_walks(&self) -> usize {
        self.ancestry_walks
    }

    /// How many exhaustive derivations have been requested.
    #[must_use]
    pub fn exhaustive_derivations(&self) -> usize {
        self.exhaustive_derivations
    }

    /// The view commit `commit` maps to, if this cache has already derived it.
    ///
    /// `Some(None)` means the commit was derived and has no counterpart,
    /// because nothing of the filtered path exists in its ancestry.
    #[must_use]
    pub fn derived(&self, commit: &oid, filter: &Filter) -> Option<Option<ObjectId>> {
        self.per_filter.get(filter)?.commits.get(commit).copied()
    }

    /// The view commit `commit` would have been, had the view not dropped it.
    ///
    /// `None` for a commit this cache has not derived, and for one the view
    /// keeps -- [`Self::derived`] is the answer for the second. Set only by
    /// deriving `commit`, so ask after [`derive()`] rather than before.
    ///
    /// This exists because "the view does not contain this commit" and "the
    /// view drops this commit" are the same answer from [`derive()`] and
    /// opposite answers to the question a caller syncing with a published
    /// repository is asking. See [`verify::integrated`].
    #[must_use]
    pub fn elided(&self, commit: &oid, filter: &Filter) -> Option<ObjectId> {
        self.per_filter.get(filter)?.elided.get(commit).copied()
    }

    /// Validates and seeds one source-to-view anchor for an incremental derive.
    ///
    /// Validation reads objects but writes none. The source and view commits
    /// must both exist, the source must be an ancestor of `revision`, and the
    /// filtered source tree must equal the view commit's tree. Only after all
    /// checks pass is the pair added to this in-memory cache.
    pub fn seed_anchor(
        &mut self,
        repo: &gix::Repository,
        revision: &oid,
        filter: &Filter,
        anchor: DeriveAnchor,
    ) -> Result<(), Error> {
        let view_tree = commit_tree(repo, &anchor.view)?;
        self.seed_anchor_with_tree(repo, revision, filter, anchor, view_tree)
    }

    /// Seeds an anchor whose source ancestry was already validated for the
    /// caller's revision.
    ///
    /// The source and view objects and their filtered trees are still checked.
    /// The caller must bind its ancestry proof to both commit ids and the exact
    /// revision being derived.
    pub fn seed_anchor_after_ancestry_check(
        &mut self,
        repo: &gix::Repository,
        filter: &Filter,
        anchor: DeriveAnchor,
    ) -> Result<(), Error> {
        let view_tree = commit_tree(repo, &anchor.view)?;
        self.store_anchor(repo, filter, anchor, view_tree)
    }

    /// Validates an anchor commit fetched without its old ancestry.
    ///
    /// `view_commit` is hashed before it is trusted. The object is written only
    /// after the source ancestry and tree checks pass, so this validation
    /// leaves the repository unchanged.
    pub fn validate_fetched_anchor(
        &mut self,
        repo: &gix::Repository,
        revision: &oid,
        filter: &Filter,
        anchor: DeriveAnchor,
        view_commit: &[u8],
    ) -> Result<(), Error> {
        let actual =
            gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Commit, view_commit)?;
        if actual != anchor.view {
            return Err(Error::AnchorObjectMismatch {
                expected: anchor.view,
                actual,
            });
        }
        let view_tree = gix::objs::CommitRef::from_bytes(view_commit, repo.object_hash())
            .map_err(|_| Error::MalformedCommit)?
            .tree();
        self.seed_anchor_with_tree(repo, revision, filter, anchor, view_tree)
    }

    /// Validates and seeds fetched anchor bytes after the caller checked that
    /// the source is an ancestor of its exact revision.
    pub fn validate_fetched_anchor_after_ancestry_check(
        &mut self,
        repo: &gix::Repository,
        filter: &Filter,
        anchor: DeriveAnchor,
        view_commit: &[u8],
    ) -> Result<(), Error> {
        let actual =
            gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Commit, view_commit)?;
        if actual != anchor.view {
            return Err(Error::AnchorObjectMismatch {
                expected: anchor.view,
                actual,
            });
        }
        let view_tree = gix::objs::CommitRef::from_bytes(view_commit, repo.object_hash())
            .map_err(|_| Error::MalformedCommit)?
            .tree();
        self.store_anchor(repo, filter, anchor, view_tree)
    }

    /// Creates a parentless anchor after the caller checked source ancestry.
    ///
    /// A new full Git remote rejects a shallow merge anchor until every parent
    /// object exists there. Removing the source commit's parents gives the view
    /// a self-contained root while preserving its filtered tree and every other
    /// commit byte. The manifest's expected id is checked before anything is
    /// written. The caller must bind its ancestry proof to `anchor.source` and
    /// the exact revision being derived.
    pub fn create_root_anchor_after_ancestry_check(
        &mut self,
        repo: &gix::Repository,
        filter: &Filter,
        anchor: DeriveAnchor,
    ) -> Result<(), Error> {
        let (bytes, actual, view_tree) = root_anchor(repo, &anchor.source, filter)?;
        if actual != anchor.view {
            return Err(Error::AnchorObjectMismatch {
                expected: anchor.view,
                actual,
            });
        }
        self.store_anchor(repo, filter, anchor, view_tree)?;
        repo.objects.write_buf(gix::objs::Kind::Commit, &bytes)?;
        Ok(())
    }

    fn seed_anchor_with_tree(
        &mut self,
        repo: &gix::Repository,
        revision: &oid,
        filter: &Filter,
        anchor: DeriveAnchor,
        view_tree: ObjectId,
    ) -> Result<(), Error> {
        if !self.reachable.contains_key(revision) {
            let reachable = reachable_commits(repo, revision)?;
            self.reachable.insert(revision.to_owned(), reachable);
            self.ancestry_walks += 1;
        }
        if !self
            .reachable
            .get(revision)
            .is_some_and(|reachable| reachable.contains(&anchor.source))
        {
            return Err(Error::AnchorNotAncestor {
                anchor_source: anchor.source,
                revision: revision.to_owned(),
            });
        }
        self.store_anchor(repo, filter, anchor, view_tree)
    }

    fn store_anchor(
        &mut self,
        repo: &gix::Repository,
        filter: &Filter,
        anchor: DeriveAnchor,
        view_tree: ObjectId,
    ) -> Result<(), Error> {
        let source_raw = read_commit(repo, &anchor.source)?;
        let source_tree = gix::objs::CommitRef::from_bytes(&source_raw, repo.object_hash())
            .map_err(|_| Error::MalformedCommit)?
            .tree();
        let filtered_tree = filtered_tree(repo, &source_tree, filter)?
            .unwrap_or_else(|| ObjectId::empty_tree(repo.object_hash()));
        if filtered_tree != view_tree {
            return Err(Error::AnchorTreeMismatch {
                anchor_source: anchor.source,
                view: anchor.view,
                source_tree: Box::new(filtered_tree),
                view_tree,
            });
        }

        let per_filter = self.per_filter.entry(filter.clone()).or_default();
        per_filter.commits.insert(anchor.source, Some(anchor.view));
        per_filter.source_trees.insert(anchor.source, source_tree);
        per_filter.view_trees.insert(anchor.view, view_tree);
        per_filter.grafts.insert(anchor.view, anchor.source);
        Ok(())
    }
}

fn root_anchor(
    repo: &gix::Repository,
    source: &oid,
    filter: &Filter,
) -> Result<(Vec<u8>, ObjectId, ObjectId), Error> {
    let source_raw = read_commit(repo, source)?;
    let source = gix::objs::CommitRef::from_bytes(&source_raw, repo.object_hash())
        .map_err(|_| Error::MalformedCommit)?;
    let view_tree = filtered_tree(repo, &source.tree(), filter)?
        .unwrap_or_else(|| ObjectId::empty_tree(repo.object_hash()));
    let bytes = raw::replace_ids(&source_raw, &view_tree, &[])?;
    let id = gix::objs::compute_hash(repo.object_hash(), gix::objs::Kind::Commit, &bytes)?;
    Ok((bytes, id, view_tree))
}

/// Derives the view commit for `commit` under `filter`.
///
/// Returns `None` when no commit reachable from `commit` contains the filtered
/// path at all, since there is then no history to show.
///
/// This is a pure function of `(commit, filter)` and the contents of `repo`.
/// It writes the objects it produces into `repo`, which for a derived view is
/// the same store the parent lives in.
pub fn derive(
    repo: &gix::Repository,
    commit: &oid,
    filter: &Filter,
    cache: &mut Cache,
) -> Result<Option<ObjectId>, Error> {
    cache.exhaustive_derivations += 1;
    let per_filter = cache.per_filter.entry(filter.clone()).or_default();
    Derivation {
        repo,
        filter,
        cache: per_filter,
        prune_irrelevant_parents: false,
    }
    .run(commit)
}

/// Derives only the requested view tip, without filling the cache for parent
/// histories which cannot affect that tip.
///
/// This returns the same commit as [`derive()`]. Its cache is not an exhaustive
/// record of elided commits in skipped parent histories, so callers which read
/// [`Cache::elided`] for every reachable source commit must use [`derive()`].
pub fn derive_tip(
    repo: &gix::Repository,
    commit: &oid,
    filter: &Filter,
    cache: &mut Cache,
) -> Result<Option<ObjectId>, Error> {
    let per_filter = cache.per_filter.entry(filter.clone()).or_default();
    Derivation {
        repo,
        filter,
        cache: per_filter,
        prune_irrelevant_parents: true,
    }
    .run(commit)
}

/// Lifts `commit`, a commit of a view, back into the parent repository on top
/// of `onto`.
///
/// The result keeps `commit`'s metadata verbatim. Its parents are the parent
/// repo counterparts of `commit`'s parents where `cache` knows them, which is
/// the case when `commit` came out of [`derive()`] or an earlier `unfilter`
/// with the same cache; a root or an unknown single parent lands on `onto`.
///
/// A parent that `onto` *itself* derives to becomes `onto`. Several parent-repo
/// commits can derive to one view commit, and the cache remembers the earliest,
/// so without this a lift onto a monorepo tip would attach to the vendored
/// commit that first produced the view instead of to the tip the caller named.
/// The two are the same position in the view, and re-deriving is exact only
/// from the one the caller asked for.
///
/// Its tree is built on its parents' counterparts with the filtered path
/// replaced, falling back to `onto`'s tree only when no parent has a
/// counterpart. One counterpart makes the lift a patch on it: `onto` positions
/// a commit whose ancestry is unknown here, it does not pull in monorepo
/// changes that the commit's own parent does not have, and putting a lifted
/// patch on top of a monorepo that has moved is two operations, this one and
/// then a merge. Several counterparts make the lift a merge of them, so its
/// tree outside the filtered path is their three way merge; a path two
/// counterparts changed differently is [`Error::OuterConflict`], never a
/// silent pick of one side.
///
/// `cache` is updated so a later `unfilter` of a descendant finds this commit.
/// Applying this over an entire history, parents first with `onto` set to the
/// previous result, is exactly the prefix injection that round trip identity is
/// about.
///
/// # Errors
///
/// Prefix filters cannot conflict on content, since the result is a pure tree
/// overlay. They can conflict on topology: a merge in the view whose sides were
/// grafted into unrelated places in the parent has no single answer, and a side
/// with no counterpart at all surfaces as [`Error::Ungrafted`].
pub fn unfilter(
    repo: &gix::Repository,
    commit: &oid,
    onto: &oid,
    filter: &Filter,
    cache: &mut Cache,
) -> Result<ObjectId, Error> {
    let per_filter = cache.per_filter.entry(filter.clone()).or_default();
    let raw = read_commit(repo, commit)?;
    let parsed = gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
        .map_err(|_| Error::MalformedCommit)?;

    // `onto` wins over the graft map for a parent it derives to. The graft map
    // remembers the FIRST parent-repo commit that produced a view commit, which
    // after an import is the injected commit deep in the vendored lineage, not
    // the monorepo tip that also derives to it. Lifting onto that injected
    // commit puts the result beside the monorepo rather than on it, and the
    // merge needed to bring it back reverse-maps into a merge commit in the
    // view that the person who wrote the change never wrote, one per round
    // trip. Where `onto` derives to the same view commit the two are
    // interchangeable as ancestry, so the caller's choice is the useful one.
    // Where it does not, `onto` is genuinely a different position and the graft
    // map is the only correct answer; merging forward is then the caller's
    // second step, which is the case the tree comment below is about.
    // Only under rules that say so. V1 lifting takes whichever counterpart the
    // cache learned first, and old variants keep their behavior forever even
    // where, as here, that behavior was an accident of call order.
    let onto_view = match filter.semantics {
        Semantics::V1 => None,
        Semantics::V2 => per_filter.commits.get(&onto.to_owned()).copied().flatten(),
    };
    let mut parents = Vec::with_capacity(parsed.parents.len());
    for parent in parsed.parents() {
        if onto_view == Some(parent) {
            parents.push(onto.to_owned());
            continue;
        }
        match per_filter.grafts.get(&parent) {
            Some(grafted) => parents.push(*grafted),
            // A single unknown parent means the view commit sits on history
            // this cache did not inject, so `onto` is the only position it can
            // take. A merge has no such fallback.
            None if parsed.parents.len() == 1 => parents.push(onto.to_owned()),
            None => return Err(Error::Ungrafted { id: parent }),
        }
    }

    // The base for the tree is built from the parents' counterparts, and only
    // with no counterpart at all from `onto`. Taking `onto`'s tree while
    // parenting the result on a different commit is not a graft, it is a
    // fabrication: the result would carry every change `onto` made outside the
    // prefix while naming a parent that does not contain them, so those
    // changes would look like this commit's own work and `onto` would be an
    // ancestor of nothing. Lifting a patch and merging the monorepo forward
    // are two operations, and conflating them is how reverse-applying a
    // rewritten view ends up rewriting the monorepo.
    //
    // One parent keeps that patch rule. A merge is the opposite case: the
    // lifted commit names every counterpart as an ancestor, so a tree that is
    // just the first counterpart's tree reverts, outside the prefix,
    // everything the other sides' histories did, and a later three way merge
    // against one of those sides reads exactly that as a deliberate revert and
    // applies it. That is how a lift built on a stale branch point deletes
    // files the integration target already has. So a merge's outer halves are
    // merged, three way against the counterparts' merge base, and a path both
    // sides changed differently surfaces as [`Error::OuterConflict`] rather
    // than one side silently winning.
    let base_tree = outer_base_tree(repo, &parents, onto, &filter.components)?;
    let tree = graft_tree(repo, &base_tree, &filter.components, &parsed.tree())?;
    let bytes = raw::replace_ids(&raw, &tree, &parents)?;
    let id = repo.objects.write_buf(gix::objs::Kind::Commit, &bytes)?;
    per_filter.grafts.insert(commit.to_owned(), id);
    per_filter
        .view_trees
        .insert(commit.to_owned(), parsed.tree());
    // Deliberately *not* recording `id -> commit` in `commits`. That the
    // injected commit derives back to `commit` is the claim this crate exists
    // to make good on, not an assumption it may seed itself with; caching it
    // here would turn any round trip check into a lookup of its own input.
    Ok(id)
}

/// Replaces what `filter` keeps of `base_tree` with `subtree`, returning the
/// grafted root tree.
///
/// This is the tree half of adopting an upstream repository: `jj views add`
/// builds a host commit whose filtered tree must equal the adopted commit's
/// tree, and it has to build that commit at the tree level. Materializing the
/// adopted snapshot through a working copy silently loses a tracked file that
/// a `.gitignore` also names -- git keeps such a file, a checkout-and-snapshot
/// round trip does not -- and the loss stays invisible until anchor validation
/// compares trees. Grafting the tree object directly cannot drop anything, and
/// filtering the result returns `subtree` by construction, which is exactly
/// the equality [`Cache::seed_anchor`] checks when the recorded anchor is
/// later trusted.
pub fn graft_snapshot(
    repo: &gix::Repository,
    base_tree: &oid,
    filter: &Filter,
    subtree: &oid,
) -> Result<ObjectId, Error> {
    graft_tree(repo, base_tree, &filter.components, subtree)
}

/// The tree a lifted commit is built on, before the view's own subtree is
/// grafted over the filtered path.
///
/// No counterpart at all: `onto`'s tree, the only position the commit can
/// take. One counterpart: its tree, the lift being a patch on that parent.
/// Several: the counterparts' trees merged, three way against the merge base
/// of the respective commits, because the lifted commit names every
/// counterpart as an ancestor and a merge's tree has to contain what its
/// ancestry contains.
///
/// The subtree at `prefix` never conflicts: the view commit's own tree
/// replaces it wholesale afterwards, so whatever the merge leaves there is
/// discarded.
fn outer_base_tree(
    repo: &gix::Repository,
    parents: &[ObjectId],
    onto: &oid,
    prefix: &[BString],
) -> Result<ObjectId, Error> {
    let Some((first, rest)) = parents.split_first() else {
        return commit_tree(repo, onto);
    };
    let mut merged = commit_tree(repo, first)?;
    for parent in rest {
        let theirs = commit_tree(repo, parent)?;
        if theirs == merged {
            continue;
        }
        // For a merge of more than two parents the base of each fold is taken
        // against the first parent rather than the accumulated result, which
        // is a commit-less tree. GitHub and jj write two-parent merges, so the
        // octopus case is an approximation that at worst reports a conflict
        // where a cleverer base would not.
        let base_tree = match merge_base(repo, first, parent)? {
            Some(base) => commit_tree(repo, &base)?,
            None => ObjectId::empty_tree(repo.object_hash()),
        };
        let mut merge = OuterMerge {
            repo,
            ours_commit: *first,
            theirs_commit: *parent,
        };
        merged = merge.trees(&base_tree, &merged, &theirs, prefix, &mut Vec::new())?;
    }
    Ok(merged)
}

/// The best common ancestor of two commits, `None` when their histories are
/// unrelated.
fn merge_base(
    repo: &gix::Repository,
    one: &oid,
    two: &oid,
) -> Result<Option<ObjectId>, Error> {
    match repo.merge_base(one.to_owned(), two.to_owned()) {
        Ok(id) => Ok(Some(id.detach())),
        Err(gix::repository::merge_base::Error::NotFound { .. }) => Ok(None),
        Err(source) => Err(Error::MergeBase {
            first: one.to_owned(),
            second: two.to_owned(),
            source: Box::new(source),
        }),
    }
}

/// Three way merge of two counterparts' trees, for the outer half of a lifted
/// merge commit.
struct OuterMerge<'a> {
    repo: &'a gix::Repository,
    /// The side already accumulated, for the conflict report.
    ours_commit: ObjectId,
    /// The side being merged in, for the conflict report.
    theirs_commit: ObjectId,
}

impl OuterMerge<'_> {
    /// Merges `ours` and `theirs` against `base`, entry by entry.
    ///
    /// `prefix` is what remains of the filter path below this level. The entry
    /// it finally names is taken from `ours` without comparing, because the
    /// caller grafts the view's own tree over that path afterwards. `at` is
    /// the path walked so far, for the conflict report.
    ///
    /// Content is never merged: a file both sides changed differently is a
    /// conflict, not a candidate for line level resolution, because nothing
    /// here can ask a person and a wrong silent answer is exactly the data
    /// loss this merge exists to prevent.
    fn trees(
        &mut self,
        base: &oid,
        ours: &oid,
        theirs: &oid,
        prefix: &[BString],
        at: &mut Vec<BString>,
    ) -> Result<ObjectId, Error> {
        let base_entries = decode_entries(self.repo, base)?;
        let our_entries = decode_entries(self.repo, ours)?;
        let their_entries = decode_entries(self.repo, theirs)?;
        let signature = |entry: &gix::objs::tree::Entry| (entry.mode, entry.oid);
        let by_name = |entries: &[gix::objs::tree::Entry], name: &BString| {
            entries
                .iter()
                .find(|entry| entry.filename == *name)
                .map(signature)
        };

        // Union of the two sides' names, in either side's order; the result is
        // sorted below. A name only the base has was deleted on both sides.
        let mut names: Vec<BString> = Vec::new();
        for entry in our_entries.iter().chain(their_entries.iter()) {
            if !names.contains(&entry.filename) {
                names.push(entry.filename.clone());
            }
        }

        let mut entries: Vec<gix::objs::tree::Entry> = Vec::new();
        let mut push = |name: &BString, (mode, oid): (gix::objs::tree::EntryMode, ObjectId)| {
            entries.push(gix::objs::tree::Entry {
                mode,
                filename: name.clone(),
                oid,
            });
        };
        for name in &names {
            let b = by_name(&base_entries, name);
            let o = by_name(&our_entries, name);
            let t = by_name(&their_entries, name);
            if prefix.first() == Some(name) && prefix.len() == 1 {
                // The grafted path. The view's tree replaces it wholesale, so
                // there is nothing to compare; keeping one side's entry keeps
                // the object graph connected until the graft lands.
                if let Some(entry) = o.or(t) {
                    push(name, entry);
                }
                continue;
            }
            if o == t {
                if let Some(entry) = o {
                    push(name, entry);
                }
                continue;
            }
            if t == b {
                if let Some(entry) = o {
                    push(name, entry);
                }
                continue;
            }
            if o == b {
                if let Some(entry) = t {
                    push(name, entry);
                }
                continue;
            }
            // Both sides changed it, differently. Two trees can still agree
            // entry by entry below; anything else is a conflict.
            match (o, t) {
                (Some((our_mode, our_oid)), Some((their_mode, their_oid)))
                    if our_mode.is_tree() && their_mode.is_tree() =>
                {
                    let child_base = match b {
                        Some((mode, oid)) if mode.is_tree() => oid,
                        _ => ObjectId::empty_tree(self.repo.object_hash()),
                    };
                    let child_prefix = match prefix.first() {
                        Some(component) if component == name => &prefix[1..],
                        _ => &[],
                    };
                    at.push(name.clone());
                    let merged =
                        self.trees(&child_base, &our_oid, &their_oid, child_prefix, at)?;
                    at.pop();
                    if merged != ObjectId::empty_tree(self.repo.object_hash()) {
                        push(name, (our_mode, merged));
                    }
                }
                _ => {
                    let mut path = BString::default();
                    for component in at.iter() {
                        path.extend_from_slice(component);
                        path.push(b'/');
                    }
                    path.extend_from_slice(name);
                    return Err(Error::OuterConflict {
                        path,
                        ours: self.ours_commit,
                        theirs: self.theirs_commit,
                    });
                }
            }
        }
        // git requires tree entries sorted with directory names compared as if
        // they ended in a slash; `gix_object::tree::Entry`'s ordering does that.
        entries.sort();
        Ok(self.repo.objects.write(&gix::objs::Tree { entries })?)
    }
}

struct Derivation<'a> {
    repo: &'a gix::Repository,
    filter: &'a Filter,
    cache: &'a mut FilterCache,
    prune_irrelevant_parents: bool,
}

/// One step of the explicit traversal stack.
enum Step {
    /// Resolve the commit's parents first.
    Enter(ObjectId),
    /// Parents are resolved, so map this commit.
    Map(ObjectId),
    /// The commit's result is its first parent's result.
    MapToFirstParent(ObjectId, ObjectId),
}

impl Derivation<'_> {
    fn run(&mut self, head: &oid) -> Result<Option<ObjectId>, Error> {
        // An explicit stack rather than recursion: the histories this is for
        // run hundreds of thousands of commits deep on a single chain, which
        // overflows the default stack long before it runs out of memory.
        let mut stack = vec![Step::Enter(head.to_owned())];
        while let Some(step) = stack.pop() {
            let (id, expanded, first_parent) = match step {
                Step::Enter(id) => (id, false, None),
                Step::Map(id) => (id, true, None),
                Step::MapToFirstParent(id, first_parent) => (id, true, Some(first_parent)),
            };
            if self.cache.commits.contains_key(&id) {
                continue;
            }
            let raw = self.read_source_commit(&id)?;
            if expanded {
                if let Some(first_parent) = first_parent {
                    self.map_to_first_parent(&id, &raw, &first_parent)?;
                } else {
                    self.map(&id, &raw)?;
                }
                continue;
            }
            let parsed = gix::objs::CommitRef::from_bytes(&raw, self.repo.object_hash())
                .map_err(|_| Error::MalformedCommit)?;
            if self.prune_irrelevant_parents
                && let Some(first_parent) = self.prunable_first_parent(&parsed)?
            {
                if self.cache.commits.contains_key(&first_parent) {
                    self.map_to_first_parent(&id, &raw, &first_parent)?;
                } else {
                    stack.push(Step::MapToFirstParent(id, first_parent));
                    stack.push(Step::Enter(first_parent));
                }
                continue;
            }
            let pending: Vec<ObjectId> = parsed
                .parents()
                .filter(|parent| !self.cache.commits.contains_key(parent))
                .collect();
            if pending.is_empty() {
                self.map(&id, &raw)?;
            } else {
                stack.push(Step::Map(id));
                stack.extend(pending.into_iter().map(Step::Enter));
            }
        }
        Ok(self.cache.commits.get(head).copied().flatten())
    }

    fn read_source_commit(&mut self, id: &oid) -> Result<Vec<u8>, Error> {
        self.cache.commit_reads += 1;
        read_commit(self.repo, id)
    }

    /// Returns the first parent when later histories cannot change this tip.
    fn prunable_first_parent(
        &mut self,
        parsed: &gix::objs::CommitRef<'_>,
    ) -> Result<Option<ObjectId>, Error> {
        let sources: Vec<ObjectId> = parsed.parents().collect();
        if sources.len() < 2 || self.filter.keep_trivial_merges {
            return Ok(None);
        }
        let source_tree = parsed.tree();
        let Some(filtered_tree) = self.derive_tree(&source_tree)? else {
            return Ok(None);
        };

        let mut source_trees = Vec::with_capacity(sources.len());
        let mut filtered_trees = Vec::with_capacity(sources.len());
        for source in &sources {
            let raw = self.read_source_commit(source)?;
            let parent = gix::objs::CommitRef::from_bytes(&raw, self.repo.object_hash())
                .map_err(|_| Error::MalformedCommit)?;
            let tree = parent.tree();
            source_trees.push(tree);
            filtered_trees.push(self.derive_tree(&tree)?);
            self.cache.source_trees.insert(*source, tree);
        }

        let first_matches = filtered_trees.first() == Some(&Some(filtered_tree));
        if !first_matches {
            return Ok(None);
        }
        let all_match = filtered_trees
            .iter()
            .all(|tree| *tree == Some(filtered_tree));
        let already_empty = source_trees.iter().all(|tree| *tree == source_tree);
        let drops_unchanged = match self.filter.elide {
            Elide::Nothing => false,
            Elide::Unchanged => !already_empty,
            Elide::UnchangedIncludingAlreadyEmpty => true,
        };
        if all_match && drops_unchanged {
            return Ok(sources.first().copied());
        }

        let was_trivial = source_trees.first() == Some(&source_tree);
        let has_distinct_parent = filtered_trees
            .iter()
            .skip(1)
            .flatten()
            .any(|tree| *tree != filtered_tree);
        if !was_trivial && has_distinct_parent {
            return Ok(sources.first().copied());
        }
        Ok(None)
    }

    fn map_to_first_parent(
        &mut self,
        id: &oid,
        raw: &[u8],
        first_parent: &oid,
    ) -> Result<(), Error> {
        let parsed = gix::objs::CommitRef::from_bytes(raw, self.repo.object_hash())
            .map_err(|_| Error::MalformedCommit)?;
        self.cache.source_trees.insert(id.to_owned(), parsed.tree());
        let mapped = self.cache.commits.get(first_parent).copied().flatten();
        self.cache.commits.insert(id.to_owned(), mapped);
        Ok(())
    }

    /// Maps one commit whose parents are already mapped.
    ///
    /// The order of the decisions here follows `create_filtered_commit2` in
    /// josh's `josh-core/src/history.rs`, because the two rules that keep hash
    /// identity are both easy to get wrong in the same direction and both live
    /// in that order: a trivial merge is spared if it was already trivial, and
    /// an unchanged commit is spared if it was already empty.
    fn map(&mut self, id: &oid, raw: &[u8]) -> Result<(), Error> {
        let parsed = gix::objs::CommitRef::from_bytes(raw, self.repo.object_hash())
            .map_err(|_| Error::MalformedCommit)?;
        let source_tree = parsed.tree();
        let sources: Vec<ObjectId> = parsed.parents().collect();
        let subtree = self.derive_tree(&source_tree)?;
        self.cache.source_trees.insert(id.to_owned(), source_tree);

        // An absent path is the empty tree, not a special case. Treating it
        // uniformly is what makes a commit that *deletes* the filtered
        // directory show up in the view as a commit that empties it, rather
        // than vanishing.
        let filtered_tree =
            subtree.unwrap_or_else(|| ObjectId::empty_tree(self.repo.object_hash()));
        let parents = self.derived_parents(&sources);

        // Did this commit change the filtered path relative to any parent?
        let mut affects_filtered = false;
        for parent in &parents {
            if self.view_tree(parent)? != filtered_tree {
                affects_filtered = true;
                break;
            }
        }
        // Was the commit already empty before filtering? Vacuously true for a
        // root, matching josh, so a root is never dropped for being unchanged.
        let already_empty = sources
            .iter()
            .all(|parent| self.cache.source_trees.get(parent) == Some(&source_tree));

        let mapped = if let Some(collapsed) =
            self.trivial_merge_target(&sources, source_tree, filtered_tree, &parents)?
        {
            self.record_elided(id, raw, &filtered_tree, &parents)?;
            Some(collapsed)
        } else {
            let unchanged = !affects_filtered;
            let drop = match self.filter.elide {
                Elide::Nothing => false,
                Elide::Unchanged => unchanged && !already_empty,
                Elide::UnchangedIncludingAlreadyEmpty => unchanged,
            };
            match (drop, parents.first()) {
                // Dropped, and the parent takes its place in the view.
                (true, Some(first)) => {
                    self.record_elided(id, raw, &filtered_tree, &parents)?;
                    Some(*first)
                }
                // Dropped with nothing to fall back to, and nothing of the path
                // here, so there is no counterpart at all.
                (true, None) if subtree.is_none() => None,
                _ if subtree.is_none() && parents.is_empty() => None,
                _ => Some(self.build(id, raw, &filtered_tree, &parents)?),
            }
        };

        self.cache.commits.insert(id.to_owned(), mapped);
        Ok(())
    }

    /// Records the view commit this one would have been, had the view kept it.
    ///
    /// A dropped commit leaves no trace in the view, so [`derive()`] gives the
    /// same answer for "this was never here" and "this is here and the view
    /// drops it". Those are opposite answers to the only question a caller
    /// syncing against a published repository asks, and reading the second as
    /// the first does not merely misreport: lifting the commit produces a
    /// commit the view drops, so the next derivation cannot see it either and
    /// the next sync lifts it again, forever. ENG-11873 was three copies of one
    /// upstream merge, one per `jj views fetch` against a remote that had not
    /// moved.
    ///
    /// The id is what [`Derivation::build`] would have written, so for a commit
    /// that came from [`unfilter`] it is exactly the view commit that was
    /// lifted: lifting copies every byte but the `tree` and `parent` lines, and
    /// this puts those back. That makes the answer exact rather than a
    /// heuristic on metadata -- a match means the commit's tree, its parents
    /// and every other byte agree, which is what being the same commit is.
    ///
    /// The object is deliberately not written. It belongs to no history: the
    /// view does not contain it and the parent repository already holds the
    /// commit it came from. Writing one per dropped commit would add an
    /// unreachable object for every monorepo commit that left the prefix alone,
    /// which on a monorepo is nearly all of them.
    fn record_elided(
        &mut self,
        id: &oid,
        raw: &[u8],
        tree: &oid,
        parents: &[ObjectId],
    ) -> Result<(), Error> {
        let bytes = raw::replace_ids(raw, tree, parents)?;
        let would_be =
            gix::objs::compute_hash(self.repo.object_hash(), gix::objs::Kind::Commit, &bytes)?;
        self.cache.elided.insert(id.to_owned(), would_be);
        Ok(())
    }

    /// The view commit a trivial merge collapses onto, if it collapses.
    ///
    /// A merge whose filtered tree equals its first filtered parent's says
    /// nothing about the filtered path. It is dropped unless it was already
    /// trivial before filtering, in which case dropping it would move its hash.
    fn trivial_merge_target(
        &mut self,
        sources: &[ObjectId],
        source_tree: ObjectId,
        filtered_tree: ObjectId,
        parents: &[ObjectId],
    ) -> Result<Option<ObjectId>, Error> {
        if self.filter.keep_trivial_merges || parents.len() < 2 {
            return Ok(None);
        }
        let Some(first) = parents.first() else {
            return Ok(None);
        };
        if self.view_tree(first)? != filtered_tree {
            return Ok(None);
        }
        let was_trivial = sources
            .first()
            .and_then(|parent| self.cache.source_trees.get(parent))
            == Some(&source_tree);
        Ok((!was_trivial).then_some(*first))
    }

    /// The view commits this commit's parents map to.
    ///
    /// Two *distinct* parents can map to the same view commit once elision
    /// collapses one onto the other, and a merge with twin parents is not a
    /// merge, so that duplicate is dropped. A commit that already listed the
    /// same parent twice is a different matter: git stores and preserves it, so
    /// collapsing it would move the commit's hash for no reason. The linux
    /// kernel has four such commits and the earliest, from 2005, has 1458483 of
    /// its 1464098 commits as descendants, so a blind dedupe moves 99.6% of
    /// that history. Only duplicates the filter introduced are removed.
    ///
    /// josh does not dedupe at all here, so it can emit a merge whose parents
    /// are the same commit twice. That difference is deliberate and is part of
    /// this filter's semantics.
    fn derived_parents(&self, sources: &[ObjectId]) -> Vec<ObjectId> {
        let mut parents: Vec<ObjectId> = Vec::with_capacity(sources.len());
        for (at, source) in sources.iter().enumerate() {
            let Some(mapped) = self.cache.commits.get(source).copied().flatten() else {
                continue;
            };
            let introduced = sources.iter().take(at).any(|earlier| {
                earlier != source
                    && self.cache.commits.get(earlier).copied().flatten() == Some(mapped)
            });
            if !introduced {
                parents.push(mapped);
            }
        }
        parents
    }

    /// The tree of a view commit, from the cache where possible.
    fn view_tree(&mut self, view: &oid) -> Result<ObjectId, Error> {
        if let Some(hit) = self.cache.view_trees.get(view) {
            return Ok(*hit);
        }
        let tree = commit_tree(self.repo, view)?;
        self.cache.view_trees.insert(view.to_owned(), tree);
        Ok(tree)
    }

    fn build(
        &mut self,
        source: &oid,
        raw: &[u8],
        tree: &oid,
        parents: &[ObjectId],
    ) -> Result<ObjectId, Error> {
        // Referred to by a commit, so the object has to exist or the view
        // fails `git fsck`. git itself is happy to write it lazily.
        if *tree == *ObjectId::empty_tree(self.repo.object_hash()) {
            self.repo.objects.write(&gix::objs::Tree::default())?;
        }
        let bytes = raw::replace_ids(raw, tree, parents)?;
        let id = self
            .repo
            .objects
            .write_buf(gix::objs::Kind::Commit, &bytes)?;
        self.cache.view_trees.insert(id, tree.to_owned());
        self.cache
            .grafts
            .entry(id)
            .or_insert_with(|| source.to_owned());
        Ok(id)
    }

    /// The subtree of `tree` at the filter's path, memoized at every level.
    fn derive_tree(&mut self, tree: &oid) -> Result<Option<ObjectId>, Error> {
        self.descend(tree.to_owned(), 0)
    }

    /// Recursion depth is the filter's path depth, a handful of components, not
    /// anything that grows with the size of history.
    fn descend(&mut self, tree: ObjectId, depth: usize) -> Result<Option<ObjectId>, Error> {
        let Some(component) = self.filter.components.get(depth) else {
            return Ok(Some(tree));
        };
        if let Some(hit) = self.cache.trees.get(&(tree, depth)) {
            return Ok(*hit);
        }
        self.cache.tree_reads += 1;
        // A blob where the filter expects a directory means the path is absent
        // at this revision, not that the history is broken.
        let found = match lookup(self.repo, &tree, component.as_bstr())? {
            Some((mode, child)) if mode.is_tree() => self.descend(child, depth + 1)?,
            _ => None,
        };
        self.cache.trees.insert((tree, depth), found);
        Ok(found)
    }
}

/// Replaces the subtree at `components` inside `base` with `sub`, creating any
/// missing intermediate trees.
fn graft_tree(
    repo: &gix::Repository,
    base: &oid,
    components: &[BString],
    sub: &oid,
) -> Result<ObjectId, Error> {
    let Some((name, rest)) = components.split_first() else {
        return Ok(sub.to_owned());
    };

    let mut entries = decode_entries(repo, base)?;
    let existing = entries
        .iter()
        .position(|entry| entry.filename == name.as_bstr());
    let child_base = match existing.and_then(|at| entries.get(at)) {
        Some(entry) if entry.mode.is_tree() => entry.oid,
        // Absent, or shadowed by a blob of the same name. The graft replaces
        // it, since the filtered path is authoritative for its own subtree.
        _ => ObjectId::empty_tree(repo.object_hash()),
    };
    let child = graft_tree(repo, &child_base, rest, sub)?;

    let entry = gix::objs::tree::Entry {
        mode: gix::objs::tree::EntryKind::Tree.into(),
        filename: name.clone(),
        oid: child,
    };
    match existing.and_then(|at| entries.get_mut(at)) {
        Some(slot) => *slot = entry,
        None => entries.push(entry),
    }
    // git requires tree entries sorted with directory names compared as if
    // they ended in a slash; `gix_object::tree::Entry`'s ordering does that.
    entries.sort();
    Ok(repo.objects.write(&gix::objs::Tree { entries })?)
}

fn lookup(
    repo: &gix::Repository,
    tree: &oid,
    name: &bstr::BStr,
) -> Result<Option<(gix::objs::tree::EntryMode, ObjectId)>, Error> {
    if *tree == *ObjectId::empty_tree(repo.object_hash()) {
        return Ok(None);
    }
    let object = find(repo, tree)?;
    let decoded =
        gix::objs::TreeRef::from_bytes(&object.data, repo.object_hash()).map_err(|_| {
            Error::WrongKind {
                id: tree.to_owned(),
                expected: gix::objs::Kind::Tree,
            }
        })?;
    Ok(decoded
        .entries
        .iter()
        .find(|entry| entry.filename == name)
        .map(|entry| (entry.mode, entry.oid.to_owned())))
}

fn decode_entries(
    repo: &gix::Repository,
    tree: &oid,
) -> Result<Vec<gix::objs::tree::Entry>, Error> {
    if *tree == *ObjectId::empty_tree(repo.object_hash()) {
        return Ok(Vec::new());
    }
    let object = find(repo, tree)?;
    let decoded =
        gix::objs::TreeRef::from_bytes(&object.data, repo.object_hash()).map_err(|_| {
            Error::WrongKind {
                id: tree.to_owned(),
                expected: gix::objs::Kind::Tree,
            }
        })?;
    Ok(decoded
        .entries
        .iter()
        .map(|entry| gix::objs::tree::Entry {
            mode: entry.mode,
            filename: entry.filename.to_owned(),
            oid: entry.oid.to_owned(),
        })
        .collect())
}

/// The subtree selected by `filter`, without deriving or writing a commit.
fn filtered_tree(
    repo: &gix::Repository,
    tree: &oid,
    filter: &Filter,
) -> Result<Option<ObjectId>, Error> {
    let mut current = tree.to_owned();
    for component in &filter.components {
        current = match lookup(repo, &current, component.as_bstr())? {
            Some((mode, child)) if mode.is_tree() => child,
            _ => return Ok(None),
        };
    }
    Ok(Some(current))
}

/// A commit the view publishes that also changes paths the view does not carry.
///
/// The view sends the commit's message verbatim -- that is the whole point of
/// [`Semantics::V1`] rule 1, and it is what makes derived hashes the published
/// repository's own hashes -- while sending only the files under the prefix. A
/// commit that spans both therefore publishes prose written about work nobody
/// on the other side can see, which is how a private note reaches a public
/// repository with a correctly filtered file list attached.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MixedCommit {
    /// The commit in this repository.
    pub commit: ObjectId,
    /// The view commit it derives to.
    pub derived: ObjectId,
    /// Paths it changed that the view does not carry, deepest common ancestry
    /// with the prefix first, sorted. A directory is reported as itself rather
    /// than expanded, so this costs one tree read per prefix component and not
    /// one per changed file.
    pub outside: Vec<BString>,
}

/// Paths `commit` changed that `filter` does not carry, sorted and deduplicated.
///
/// Empty means every path the commit touched is one the view publishes, so its
/// message describes only work the view shows.
///
/// `exempt` names root-relative paths the caller has decided are view metadata
/// rather than work: they are dropped from the answer. The rule this encodes is
/// the caller's, not this crate's, which is why it is an argument.
///
/// Only a commit with exactly one parent is checked; roots and merges answer
/// empty. Both are degenerate rather than safe, and for the same reason: the
/// question this asks is what a commit *changed* off the prefix, and neither
/// has a single before-tree that makes the answer mean that. A root would be
/// diffed against nothing, so it reports the entire repository, which describes
/// the repository's creation and not a mixed edit. A merge's tree differs from
/// every parent outside the prefix whenever both sides changed anything out
/// there, so checking merges would flag every ordinary integration merge. In
/// both cases the content was authored in single-parent commits this scan
/// reaches on their own. An evil merge is the gap that leaves, and it is a
/// narrower one than a check nobody can keep green.
///
/// Measured on this rule's home repository: without the root case the scan
/// reported 92 commits, 80 of them view lift roots whose message no human
/// wrote. With it, 12, every one a real mixed edit.
pub fn changes_outside(
    repo: &gix::Repository,
    commit: &oid,
    filter: &Filter,
    exempt: &[BString],
) -> Result<Vec<BString>, Error> {
    let raw = read_commit(repo, commit)?;
    let parsed = gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
        .map_err(|_| Error::MalformedCommit)?;
    let parents: Vec<ObjectId> = parsed.parents().collect();
    let [parent] = parents.as_slice() else {
        return Ok(Vec::new());
    };
    let parent_raw = read_commit(repo, parent)?;
    let before = gix::objs::CommitRef::from_bytes(&parent_raw, repo.object_hash())
        .map_err(|_| Error::MalformedCommit)?
        .tree();
    let after = parsed.tree();
    let mut out = Vec::new();
    collect_outside(
        repo,
        &before,
        &after,
        &filter.components,
        &mut BString::from(Vec::new()),
        &mut out,
    )?;
    out.sort();
    out.dedup();
    out.retain(|path| !exempt.contains(path));
    Ok(out)
}

/// Walks the prefix chain, recording every entry that changed off it.
///
/// At each level the only entry the view carries is the next prefix component,
/// so every other changed entry is outside and is recorded without descending
/// into it. Cost is one pair of tree reads per component of the prefix,
/// whatever the size of the trees.
fn collect_outside(
    repo: &gix::Repository,
    before: &oid,
    after: &oid,
    components: &[BString],
    prefix: &mut BString,
    out: &mut Vec<BString>,
) -> Result<(), Error> {
    if before == after {
        return Ok(());
    }
    let Some((head, rest)) = components.split_first() else {
        // Everything here is under the prefix, so nothing here is outside.
        return Ok(());
    };
    let before_entries = decode_entries(repo, before)?;
    let after_entries = decode_entries(repo, after)?;
    let index: HashMap<&BString, &gix::objs::tree::Entry> = before_entries
        .iter()
        .map(|entry| (&entry.filename, entry))
        .collect();
    for entry in &after_entries {
        let was = index.get(&entry.filename);
        if entry.filename == *head {
            let before_child = match was {
                Some(old) if old.mode.is_tree() && entry.mode.is_tree() => old.oid,
                // The prefix appeared, disappeared, or stopped being a tree.
                // Either way there is no pair of subtrees to compare, and the
                // change itself is inside the view.
                _ => continue,
            };
            let mut deeper = prefix.clone();
            push_component(&mut deeper, head);
            collect_outside(repo, &before_child, &entry.oid, rest, &mut deeper, out)?;
            continue;
        }
        let unchanged = was.is_some_and(|old| old.oid == entry.oid && old.mode == entry.mode);
        if !unchanged {
            out.push(joined(prefix, &entry.filename));
        }
    }
    // Deletions: present before, gone after.
    let after_names: HashMap<&BString, ()> = after_entries
        .iter()
        .map(|entry| (&entry.filename, ()))
        .collect();
    for entry in &before_entries {
        if after_names.contains_key(&entry.filename) {
            continue;
        }
        if entry.filename == *head {
            continue;
        }
        out.push(joined(prefix, &entry.filename));
    }
    Ok(())
}

fn push_component(prefix: &mut BString, component: &BString) {
    if !prefix.is_empty() {
        prefix.push(b'/');
    }
    prefix.extend_from_slice(component);
}

fn joined(prefix: &BString, name: &BString) -> BString {
    let mut out = prefix.clone();
    push_component(&mut out, name);
    out
}

/// Commits after `ancestor` that the view publishes and that also change paths
/// outside it, parents first.
///
/// `ancestor` bounds the walk the way an anchor bounds a derivation: history
/// older than it has already been published and is not re-examined. Passing
/// `None` scans everything reachable from `tip`.
///
/// A commit the view drops is not reported. Its message is never sent, so it
/// cannot leak one, and reporting it would make the check fire on the ordinary
/// case this design already handles correctly.
pub fn mixed_commits(
    repo: &gix::Repository,
    tip: &oid,
    ancestor: Option<&oid>,
    filter: &Filter,
    exempt: &[BString],
    cache: &mut Cache,
) -> Result<Vec<MixedCommit>, Error> {
    let order = match ancestor {
        Some(ancestor) => verify::ancestry_after(repo, tip, ancestor)?,
        None => verify::ancestry(repo, tip)?,
    };
    let mut out = Vec::new();
    for commit in order {
        let Some(derived) = derive(repo, &commit, filter, cache)? else {
            continue;
        };
        if cache.elided(&commit, filter).is_some() {
            continue;
        }
        let outside = changes_outside(repo, &commit, filter, exempt)?;
        if !outside.is_empty() {
            out.push(MixedCommit {
                commit,
                derived,
                outside,
            });
        }
    }
    Ok(out)
}

fn reachable_commits(
    repo: &gix::Repository,
    descendant: &oid,
) -> Result<std::collections::HashSet<ObjectId>, Error> {
    let walk = repo
        .rev_walk([descendant.to_owned()])
        .use_commit_graph(true)
        .all()
        .map_err(|source| Error::RevisionWalk(Box::new(source)))?;
    let mut reachable = std::collections::HashSet::new();
    for commit in walk {
        let commit = commit.map_err(|source| Error::RevisionWalkCommit(Box::new(source)))?;
        reachable.insert(commit.id);
    }
    Ok(reachable)
}

fn find<'repo>(repo: &'repo gix::Repository, id: &oid) -> Result<gix::Object<'repo>, Error> {
    repo.find_object(id).map_err(|source| Error::Find {
        id: id.to_owned(),
        source: Box::new(source),
    })
}

fn read_commit(repo: &gix::Repository, id: &oid) -> Result<Vec<u8>, Error> {
    let object = find(repo, id)?;
    if object.kind != gix::objs::Kind::Commit {
        return Err(Error::WrongKind {
            id: id.to_owned(),
            expected: gix::objs::Kind::Commit,
        });
    }
    Ok(object.detach().data)
}

fn commit_tree(repo: &gix::Repository, id: &oid) -> Result<ObjectId, Error> {
    let raw = read_commit(repo, id)?;
    let parsed = gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
        .map_err(|_| Error::MalformedCommit)?;
    Ok(parsed.tree())
}
