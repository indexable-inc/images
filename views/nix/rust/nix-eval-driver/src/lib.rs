//! A Rust-native evaluation driver over `nix-eval-rs`.
//!
//! The first plank of the ship-of-Theseus direction in this repo's CLAUDE.md:
//! evaluating Nix, and instantiating a derivation, without the C++ CLI shell
//! for the paths the Rust core already owns. Three pieces, in the order the
//! charter puts them:
//!
//! * [`store`] -- where bytes go. Text ingestion into a local store, written
//!   directly, with no daemon and no `nix` subprocess.
//! * [`host`] -- the embedder. A [`nix_eval_rs::host::Host`] that answers
//!   store questions for real and refuses, by name, the ones that stay
//!   bridge-side.
//! * [`run`] -- the operations, scheduled through
//!   [`nix_eval_rs::eval::drive_concurrent`].
//!
//! The `nix-eval-driver` binary is thin argument parsing over these;
//! everything it does is callable from Rust without a process.

pub mod host;
pub mod run;
pub mod store;
