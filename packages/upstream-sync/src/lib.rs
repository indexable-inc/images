//! Shared plumbing for the de-fork upstreaming tools.
//!
//! Two binaries share this crate: `upstream-sync` (the loop driver plus the
//! read-only `drift` report) and `upstream-pr` (opens one upstream
//! contribution for one fork patch). The single source of truth for what is
//! de-forked is `lib/fork-packages.nix`; the Nix wrapper renders it to JSON
//! and points `UPSTREAM_SYNC_FORK_PACKAGES` at it. The patch series itself
//! lives in each fork repo's commit DAG ([`series`]), not in this repo.

pub mod cmd;
pub mod drift;
pub mod gh;
pub mod mapping;
pub mod series;
pub mod status;
pub mod style;
