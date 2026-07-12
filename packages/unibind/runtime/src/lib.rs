//! Boundary types exported code references at runtime.
//!
//! [`UniStream`] is the stream half of the unibind surface: an exported
//! `fn` returning `UniStream<T>` becomes an async iterator in the target
//! language, and items flow one poll per consumer request (pull-based
//! backpressure). Deliberately language-free: the per-language runtime
//! glue lives in `unibind-py-runtime` and `unibind-ex-runtime`, so this
//! crate can sit inside any binding artifact without dragging another
//! language's toolchain along.

// Defined in unibind-stream (the pyo3-free crate both sides of the Rust
// ABI link) and re-exported here so exporting crates keep one import path.
pub use unibind_stream::UniStream;

