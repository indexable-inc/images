//! Boundary types exported code references at runtime.
//!
//! [`UniStream`] is the stream half of the unibind surface: an exported
//! `fn` returning `UniStream<T>` becomes an async iterator in the target
//! language, and items flow one poll per consumer request (pull-based
//! backpressure). The `py` feature adds the Python async helpers the
//! generated glue calls into.

// Defined in unibind-stream (the pyo3-free crate both sides of the Rust
// ABI link) and re-exported here so exporting crates keep one import path.
pub use unibind_stream::UniStream;

#[cfg(feature = "py")]
pub mod py;
