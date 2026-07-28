//! Sync a source tree to a remote host, or to another checkout, using git's
//! view of the tree instead of a hand written list of globs.
//!
//! This exists because rsync's exclude language is anchored wrong by default.
//! `rsync --exclude 'result*'` is written to skip the top level `result` symlink
//! a Nix build leaves behind, but an rsync pattern with no slash in it matches
//! at every path depth, so the same flag also removed
//! `crates/codec/src/impls/result.rs` from the destination. rsync reported
//! nothing; the failure surfaced ten minutes later as
//! `error[E0583]: file not found for module 'result'` in the middle of a Rust
//! build, which points at the compiler rather than at the sync.
//!
//! Three choices here make that shape impossible:
//!
//! * the file set comes from git ([`tree::list`]), so build output is skipped
//!   because `.gitignore` says so and not because someone typed a glob;
//! * a user exclude is anchored at the sync root unless it explicitly opts out
//!   ([`filter::anchor`]), so `result*` cannot reach into a subdirectory;
//! * every exclude prints the pattern it actually matched with and how many
//!   paths it removed, so a rule that ate more than intended, or matched
//!   nothing at all, is visible in the output.

pub mod filter;
pub mod target;
pub mod transfer;
pub mod tree;
