//! The three identifiers the memo table is addressed by.
//!
//! There is one table, `(Domain, Key) -> Entry`, and one content-addressed
//! store, `ObjId -> bytes`. The store, the evaluation cache and `effect.lock`
//! are not three databases; they are three trust policies over this one pair.
//! Keeping the identifiers distinct types stops a key from being looked up as
//! an object address, which is the sort of mistake that is invisible in a
//! `[u8; 32]`-typed API.

use crate::canon;
use crate::hash::{self, Hash};
use core::fmt;

/// Names an effect operation: everything about *what is being done* that is
/// not part of the individual request.
///
/// Minted as `H("ix-domain-v1" || effect-identity || op-name ||
/// canon-version)`. The canonical-encoding version is a field rather than a
/// separate table column so that changing the encoding mints fresh domains:
/// v2 rows land beside v1 rows instead of being read as if they were v1, and
/// nobody has to migrate a lock file to stay correct.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Domain(Hash);

impl Domain {
    /// Mint a domain at the current canonical-encoding version.
    #[must_use]
    pub fn mint(effect_identity: &str, op_name: &str) -> Self {
        Self::mint_at(effect_identity, op_name, canon::VERSION)
    }

    /// Mint a domain at an explicit canonical-encoding version. Only useful
    /// for reading rows minted by an older kernel; new work uses [`mint`].
    ///
    /// [`mint`]: Domain::mint
    #[must_use]
    pub fn mint_at(effect_identity: &str, op_name: &str, canon_version: &str) -> Self {
        Self(hash::tagged(
            hash::DOMAIN_TAG,
            &[
                effect_identity.as_bytes(),
                op_name.as_bytes(),
                canon_version.as_bytes(),
            ],
        ))
    }

    #[must_use]
    pub const fn hash(&self) -> &Hash {
        &self.0
    }

    #[must_use]
    pub const fn from_hash(hash: Hash) -> Self {
        Self(hash)
    }
}

/// Names one request within a domain.
///
/// Minted as `H("ix-key-v1" || domain || canon_encode(req))`. The domain is
/// mixed in so that the same request shape under two different operations
/// cannot share a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(Hash);

impl Key {
    #[must_use]
    pub fn mint(domain: Domain, req_canon: &[u8]) -> Self {
        Self(hash::tagged(
            hash::KEY_TAG,
            &[domain.hash().as_bytes(), req_canon],
        ))
    }

    #[must_use]
    pub const fn hash(&self) -> &Hash {
        &self.0
    }

    #[must_use]
    pub const fn from_hash(hash: Hash) -> Self {
        Self(hash)
    }
}

/// The address of a stored object: `H("ix-obj-v1" || bytes)`.
///
/// This is also the hash a `Checked` policy declares. The design writes that
/// check as `blake3(output) == declared`; spelling it as the object address
/// keeps the "domain separation on every hash" rule intact and means a
/// declared hash is exactly the string a user reads out of `effect.lock` or a
/// store listing, with no second hashing convention to explain. The cost is
/// that a hash published by an upstream project is not directly usable as a
/// declaration; when real fetchers arrive they will need an explicit
/// `Declared::Foreign { algo, digest }` alongside this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjId(Hash);

impl ObjId {
    /// Address of these exact bytes.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(hash::tagged(hash::OBJ_TAG, &[bytes]))
    }

    #[must_use]
    pub const fn hash(&self) -> &Hash {
        &self.0
    }

    #[must_use]
    pub const fn from_hash(hash: Hash) -> Self {
        Self(hash)
    }
}

macro_rules! display_as_hex {
    ($($id:ty),*) => {$(
        impl fmt::Display for $id {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    )*};
}
display_as_hex!(Domain, Key, ObjId);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_do_not_collide_across_kinds() {
        // Same 32 bytes of payload, three different identifiers.
        let domain = Domain::mint("fetch", "url");
        let key = Key::mint(domain, b"");
        let obj = ObjId::of(b"");
        assert_ne!(domain.hash(), key.hash());
        assert_ne!(key.hash(), obj.hash());
        assert_ne!(domain.hash(), obj.hash());
    }

    #[test]
    fn domain_depends_on_every_field() {
        let base = Domain::mint_at("fetch", "url", "canon-v1");
        assert_ne!(base, Domain::mint_at("fetch2", "url", "canon-v1"));
        assert_ne!(base, Domain::mint_at("fetch", "url2", "canon-v1"));
        assert_ne!(base, Domain::mint_at("fetch", "url", "canon-v2"));
    }

    /// The field split is real, not a concatenation: moving a character across
    /// the boundary must change the domain.
    #[test]
    fn domain_fields_are_delimited() {
        assert_ne!(Domain::mint("fetc", "hurl"), Domain::mint("fetch", "url"));
    }

    #[test]
    fn key_is_scoped_to_its_domain() {
        let one = Domain::mint("fetch", "url");
        let other = Domain::mint("fetch", "git");
        assert_ne!(Key::mint(one, b"req"), Key::mint(other, b"req"));
    }
}
