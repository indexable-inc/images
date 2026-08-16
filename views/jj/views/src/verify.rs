//! Checking that a derived view is the standalone history, hash for hash.
//!
//! This lives in the library rather than in the command because two callers
//! need it: `jj-views verify`, and any test that has just published a view and
//! wants to know whether what came back is the same history. A second
//! comparison written for the second caller is a second thing to get wrong, and
//! the failure mode is silent agreement.

use std::collections::HashSet;

use gix::ObjectId;
use gix::hash::oid;

use crate::Cache;
use crate::Error;
use crate::Filter;

/// What comparing a derived view against a standalone history found.
///
/// Every field is a set difference, so the passing state of each is an absence.
/// [`Self::identical`] is the whole answer; the samples exist to say what went
/// wrong, not to be checked one at a time.
#[derive(Clone, Debug)]
pub struct Report {
    /// The view of the revision that was derived, absent when nothing in its
    /// ancestry touched the filtered path.
    pub tip: Option<ObjectId>,
    /// The standalone tip every derived hash was required to match.
    pub expected_tip: ObjectId,
    /// How many commits the standalone history has.
    pub expected: usize,
    /// Standalone commits the view did not produce.
    pub missing: Vec<ObjectId>,
    /// View commits the standalone history does not have.
    pub extra: Vec<ObjectId>,
}

impl Report {
    /// Whether the derived history is the standalone history.
    pub fn identical(&self) -> bool {
        self.missing.is_empty() && self.extra.is_empty()
    }

    /// Whether the derived tip is the standalone tip.
    ///
    /// A commit hash covers its parents transitively, so this alone means the
    /// whole reachable history matched byte for byte. It is reported separately
    /// because it is the one hash that makes the others follow.
    pub fn tip_matches(&self) -> bool {
        self.tip.as_ref() == Some(&self.expected_tip)
    }
}

/// Derives every commit reachable from `rev` and compares the result against
/// the history reachable from `against`.
pub fn verify(
    repo: &gix::Repository,
    rev: &oid,
    against: &oid,
    filter: &Filter,
    cache: &mut Cache,
) -> Result<Report, Error> {
    let derived = derived_set(repo, rev, filter, cache)?;
    let expected = ancestry(repo, against)?;

    let missing: Vec<ObjectId> = expected
        .iter()
        .filter(|id| !derived.contains(*id))
        .copied()
        .collect();
    let expected_set: HashSet<ObjectId> = expected.iter().copied().collect();
    let extra: Vec<ObjectId> = derived
        .iter()
        .filter(|id| !expected_set.contains(*id))
        .copied()
        .collect();

    Ok(Report {
        tip: crate::derive(repo, rev, filter, cache)?,
        expected_tip: against.to_owned(),
        expected: expected.len(),
        missing,
        extra,
    })
}

/// Every view commit the parent revision's history already accounts for.
///
/// Deriving each commit is also what populates the graft map
/// [`crate::unfilter`] reads, so this is both the answer to "has this been
/// injected already" and the setup a lift needs.
pub fn derived_set(
    repo: &gix::Repository,
    tip: &oid,
    filter: &Filter,
    cache: &mut Cache,
) -> Result<HashSet<ObjectId>, Error> {
    let mut seen = HashSet::new();
    for commit in ancestry(repo, tip)? {
        if let Some(view) = crate::derive(repo, &commit, filter, cache)? {
            seen.insert(view);
        }
    }
    Ok(seen)
}

/// What a parent revision's history holds of a view, split by whether the view
/// shows it.
///
/// The two sets answer different questions and the difference is the whole
/// reason this type exists. [`Self::derived`] is what the view *is*, which is
/// what an identity check compares. [`Self::elided`] is what the parent
/// repository also holds but the view drops, which is invisible to
/// [`crate::derive`] and yet just as integrated.
#[derive(Clone, Debug, Default)]
pub struct Integrated {
    /// View commits the view itself has.
    pub derived: HashSet<ObjectId>,
    /// View commits the parent repository holds as commits the view drops.
    ///
    /// Empty under [`crate::Elide::Nothing`], which drops nothing.
    pub elided: HashSet<ObjectId>,
}

impl Integrated {
    /// Whether the parent revision's history holds `view` at all, shown or not.
    ///
    /// This is the question to ask before fetching a published repository's
    /// commit, because a commit already here and dropped by the view needs no
    /// fetching and cannot be made visible by fetching it again.
    #[must_use]
    pub fn contains(&self, view: &ObjectId) -> bool {
        self.derived.contains(view) || self.elided.contains(view)
    }
}

/// Everything of a view that `tip`'s history holds, shown or dropped.
///
/// The traversal is [`derived_set`]'s, with the cache's record of each dropped
/// commit read off as it goes, so this costs one derivation and not two.
/// [`derived_set`] stays the narrower answer because that is the one an
/// identity check wants: counting a dropped commit as derived would let
/// [`verify`] pass on a history the view does not actually produce.
pub fn integrated(
    repo: &gix::Repository,
    tip: &oid,
    filter: &Filter,
    cache: &mut Cache,
) -> Result<Integrated, Error> {
    let mut out = Integrated::default();
    for commit in ancestry(repo, tip)? {
        if let Some(view) = crate::derive(repo, &commit, filter, cache)? {
            out.derived.insert(view);
        }
        // Derived just above, so this is a lookup rather than a second walk.
        if let Some(dropped) = cache.elided(&commit, filter) {
            out.elided.insert(dropped);
        }
    }
    Ok(out)
}

/// Everything a revision added to a view after a validated source anchor.
///
/// The anchor itself is included as a derived commit. Its ancestors are not
/// enumerated or cached. A merged side that does not pass through the anchor is
/// still traversed, because that history is new to the anchored lineage.
pub fn integrated_after_anchor(
    repo: &gix::Repository,
    tip: &oid,
    anchor: crate::DeriveAnchor,
    filter: &Filter,
    cache: &mut Cache,
) -> Result<Integrated, Error> {
    let mut out = Integrated::default();
    out.derived.insert(anchor.view);
    for commit in ancestry_after(repo, tip, &anchor.source)? {
        if let Some(view) = crate::derive(repo, &commit, filter, cache)? {
            out.derived.insert(view);
        }
        if let Some(dropped) = cache.elided(&commit, filter) {
            out.elided.insert(dropped);
        }
    }
    Ok(out)
}

/// Commits reachable from `tip` after `ancestor`, parents before children.
///
/// Traversal stops at `ancestor`, so its older history is not materialized.
/// A side merged after the anchor is included even when that side does not
/// itself descend from the anchor.
pub fn ancestry_after(
    repo: &gix::Repository,
    tip: &oid,
    ancestor: &oid,
) -> Result<Vec<ObjectId>, Error> {
    let mut order: Vec<ObjectId> = Vec::new();
    let mut done: HashSet<ObjectId> = HashSet::new();
    let mut found = false;
    let mut stack: Vec<(ObjectId, bool)> = vec![(tip.to_owned(), false)];
    while let Some((id, expanded)) = stack.pop() {
        if *id == *ancestor {
            found = true;
            continue;
        }
        if done.contains(&id) {
            continue;
        }
        if expanded {
            done.insert(id);
            order.push(id);
            continue;
        }
        let raw = crate::read_commit(repo, &id)?;
        let parents: Vec<ObjectId> = gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
            .map_err(|_| Error::MalformedCommit)?
            .parents()
            .filter(|parent| !done.contains(parent))
            .collect();
        stack.push((id, true));
        stack.extend(parents.into_iter().map(|parent| (parent, false)));
    }
    if !found {
        return Err(Error::AnchorNotAncestor {
            anchor_source: ancestor.to_owned(),
            revision: tip.to_owned(),
        });
    }
    Ok(order)
}

/// Everything reachable from `tip`, parents before children.
pub fn ancestry(repo: &gix::Repository, tip: &oid) -> Result<Vec<ObjectId>, Error> {
    let mut order: Vec<ObjectId> = Vec::new();
    let mut done: HashSet<ObjectId> = HashSet::new();
    // An explicit stack rather than recursion, for the same reason the filter
    // itself uses one: a real history is deep enough on a single chain to
    // overflow the stack long before it runs out of memory.
    let mut stack: Vec<(ObjectId, bool)> = vec![(tip.to_owned(), false)];
    while let Some((id, expanded)) = stack.pop() {
        if done.contains(&id) {
            continue;
        }
        if expanded {
            done.insert(id);
            order.push(id);
            continue;
        }
        let raw = crate::read_commit(repo, &id)?;
        let parents: Vec<ObjectId> = gix::objs::CommitRef::from_bytes(&raw, repo.object_hash())
            .map_err(|_| Error::MalformedCommit)?
            .parents()
            .filter(|parent| !done.contains(parent))
            .collect();
        stack.push((id, true));
        stack.extend(parents.into_iter().map(|parent| (parent, false)));
    }
    Ok(order)
}
