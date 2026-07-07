//! Render host-language files from the unibind IR embedded in a compiled
//! artifact.
//!
//! `unibind-gen` is the out-of-process half of the unibind pipeline: the
//! macros serialize each [`unibind_core::ir::Interface`] into a link section
//! of the built artifact ([`unibind_core::embed`]), and this crate reads the
//! section back and renders the host-language surface (`.pyi` stubs,
//! `py.typed`, wrapper modules) with no Rust source in sight. The binary
//! front-end lives in `main.rs`; the library surface exists so the
//! integration tests can exercise the parsing and emission seams directly.

pub mod artifact;
pub mod host;
pub mod py;
