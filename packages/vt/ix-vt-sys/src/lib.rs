//! Raw FFI bindings to [libghostty-vt], ghostty's terminal VT engine.
//!
//! This crate is the unsafe, mechanically generated layer: a 1:1 mapping of the
//! `ghostty/vt.h` C surface produced by [`bindgen`]. It carries no domain logic
//! and performs no safety checks. Prefer the `ix-vt` crate, which wraps these
//! symbols in a safe, owned API.
//!
//! The symbols are resolved at link time against the self-contained
//! `libghostty-vt` dynamic library. See `build.rs` for how the library
//! directory is supplied (`IX_VT_GHOSTTY_LIB_DIR`).
//!
//! [libghostty-vt]: https://ghostty.org/
//! [`bindgen`]: https://github.com/rust-lang/rust-bindgen

mod bindings;

pub use bindings::*;
