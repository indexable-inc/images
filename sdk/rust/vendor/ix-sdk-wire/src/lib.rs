//! Trivial stub body for the `ix-sdk-wire` metadata stub.
//!
//! This source is intentionally minimal and is NEVER built into the public
//! SDK's closure: `buildWorkspace` injects the real prebuilt `ix-sdk-wire`
//! rlib+rmeta (fetched from R2) over this unit via `extraUnits`, and the
//! cargo-unit hash is source-independent, so only this crate's Cargo metadata
//! matters. See ../Cargo.toml for why the metadata must stay faithful to the
//! real crate, and `sdk/rust/default.nix` for the injection wiring.
