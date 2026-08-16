//! The memo table and the trust vocabulary over it.
//!
//! One table, `(Domain, Key) -> Entry`. What separates the store from the
//! evaluation cache from `effect.lock` is not where the row lives but which
//! [`Policy`] governs it and what [`Provenance`] the row therefore carries.
//!
//! `BTreeMap`, not `HashMap`: iteration order is observable here. It decides
//! the byte order of a saved lock file and the order of any digest taken over
//! the table, and a hash-order iteration would make both depend on a random
//! seed.

use crate::hash::Hash;
use crate::id::{Domain, Key, ObjId};
use std::collections::BTreeMap;
use std::time::Duration;

/// How much the kernel is allowed to trust a memoised answer, and what it must
/// do on a miss.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The effect is a pure function of its request. A miss re-performs, and
    /// re-performing is always safe, so no record beyond the row is needed.
    Keyed,
    /// The effect is not trusted to be stable, but its result is declared in
    /// advance. A miss performs once and the output must hash to `declared`.
    Checked { declared: Hash },
    /// The effect is not reproducible at all, so the first answer is the
    /// answer: trust on first use, recorded in `effect.lock`. Under
    /// [`KernelConfig::frozen`] a miss is an error instead, which is what
    /// makes a locked build refuse to invent a new pin.
    ///
    /// [`KernelConfig::frozen`]: crate::KernelConfig::frozen
    Pinned(RefreshPolicy),
    /// The effect is observable but has no value: logging, tracing, a
    /// progress bar. Always performed, never memoised.
    Transparent,
}

/// When a pinned row is allowed to be re-taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefreshPolicy {
    /// Only when a human removes the row.
    Manual,
    /// When the request changes. This needs no enforcement code: a changed
    /// request mints a different [`Key`], which is a miss, which re-performs.
    OnKeyChange,
    /// After the row has aged out.
    ///
    /// Not yet enforced. Enforcing it needs a clock, and the kernel
    /// deliberately has none (see [`crate::dispatch::PerformCtx`]); the
    /// duration is recorded so that the caller that owns the clock can expire
    /// rows itself, and so that turning this on later is not a format change.
    Ttl(Duration),
}

/// Why an output is believed. Answers "who says so" for any row a person is
/// looking at during an audit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// Re-performing would produce this again.
    Deterministic,
    /// Checked against a hash declared before the effect ran.
    Verified { declared: Hash },
    /// A human took responsibility for it. `when` is an RFC 3339 timestamp
    /// supplied by the caller rather than read from a clock here.
    Blessed {
        who: String,
        when: String,
        sig: Option<Vec<u8>>,
    },
    /// Nobody vouches for it. Reachable for rows recovered from a source that
    /// carried no provenance at all.
    None,
}

/// One row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub output: ObjId,
    pub policy: Policy,
    pub provenance: Provenance,
}

/// `(Domain, Key) -> Entry`.
///
/// The domain is the outer level so that everything about one effect is
/// contiguous, which is what makes "show me every pin for this fetcher" and
/// the grouped layout of `effect.lock` a walk rather than a scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoTable {
    rows: BTreeMap<Domain, BTreeMap<Key, Entry>>,
}

impl MemoTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn get(&self, domain: Domain, key: Key) -> Option<&Entry> {
        self.rows.get(&domain).and_then(|keys| keys.get(&key))
    }

    /// Insert or replace a row. Returns the entry that was there.
    pub fn insert(&mut self, domain: Domain, key: Key, entry: Entry) -> Option<Entry> {
        self.rows.entry(domain).or_default().insert(key, entry)
    }

    pub fn remove(&mut self, domain: Domain, key: Key) -> Option<Entry> {
        // Drop the domain once its last key goes, so an emptied table compares
        // equal to a fresh one and does not save a header with no rows.
        let keys = self.rows.get_mut(&domain)?;
        let removed = keys.remove(&key);
        if keys.is_empty() {
            self.rows.remove(&domain);
        }
        removed
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.values().map(BTreeMap::len).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.values().all(BTreeMap::is_empty)
    }

    /// Every row, domain-major then key, in the order a lock file wants them.
    pub fn iter(&self) -> impl Iterator<Item = (Domain, Key, &Entry)> {
        self.rows
            .iter()
            .flat_map(|(domain, keys)| keys.iter().map(move |(key, entry)| (*domain, *key, entry)))
    }

    /// Domains that have at least one row.
    pub fn domains(&self) -> impl Iterator<Item = Domain> {
        self.rows.keys().copied()
    }
}

/// Kernel-wide switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KernelConfig {
    /// Refuse to mint new pins. A frozen kernel can replay `effect.lock` and
    /// nothing else, so a build either reproduces from what is recorded or
    /// fails naming what was missing. This is the switch CI runs with.
    pub frozen: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(bytes: &[u8]) -> Entry {
        Entry {
            output: ObjId::of(bytes),
            policy: Policy::Keyed,
            provenance: Provenance::Deterministic,
        }
    }

    #[test]
    fn rows_are_scoped_by_domain() {
        let one = Domain::mint("e", "a");
        let other = Domain::mint("e", "b");
        let key = Key::mint(one, b"req");
        let mut table = MemoTable::new();
        table.insert(one, key, entry(b"x"));
        assert!(table.get(one, key).is_some());
        assert!(table.get(other, key).is_none());
    }

    #[test]
    fn emptied_table_equals_a_fresh_one() {
        let domain = Domain::mint("e", "a");
        let key = Key::mint(domain, b"req");
        let mut table = MemoTable::new();
        table.insert(domain, key, entry(b"x"));
        assert_eq!(table.len(), 1);
        assert!(table.remove(domain, key).is_some());
        assert_eq!(table, MemoTable::new());
        assert!(table.is_empty());
    }

    #[test]
    fn iteration_is_domain_major_and_sorted() {
        let mut table = MemoTable::new();
        let mut expected = Vec::new();
        for effect in ["b", "a", "c"] {
            let domain = Domain::mint(effect, "op");
            for req in [b"2", b"1"] {
                let key = Key::mint(domain, req);
                table.insert(domain, key, entry(req));
                expected.push((domain, key));
            }
        }
        expected.sort();
        let seen: Vec<_> = table.iter().map(|(d, k, _)| (d, k)).collect();
        assert_eq!(seen, expected);
    }
}
