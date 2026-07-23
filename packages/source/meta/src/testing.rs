//! Assertions shared by adapter test suites.
//!
//! Lives in the production crate because every adapter already depends on
//! `source-meta` and Cargo exposes regular dependencies to `tests/` targets;
//! the module is plain assertion code with no test-framework dependency.

use crate::Document;

/// Assert two runs of an adapter over the same corpus yielded the same records
/// with identical content hashes.
///
/// The hash is the change-detection key, so a nondeterministic one would
/// re-embed an unchanged corpus every ingest.
///
/// # Panics
/// Panics when the runs differ in length, identity, or hash.
pub fn assert_deterministic_hashes(first: &[Document], second: &[Document]) {
    assert_eq!(first.len(), second.len());
    for (a, b) in first.iter().zip(second) {
        assert_eq!(a.external_id, b.external_id);
        assert_eq!(a.content_hash, b.content_hash, "stable hash across runs");
    }
}
