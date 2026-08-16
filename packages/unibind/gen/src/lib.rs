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
//!
//! One target per module, except the two JavaScript ones: `ts` (napi in node)
//! and `wasm` (`wasm-bindgen` in a browser) publish one surface, so they are
//! both flavors of the shared `js` renderers.

pub mod artifact;
pub mod ex;
pub mod host;
mod js;
pub mod jvm;
mod literal;
pub mod py;
pub mod ts;
pub mod wasm;
