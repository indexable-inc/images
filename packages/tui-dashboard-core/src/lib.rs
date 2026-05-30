//! Engine-free wire types and the browser-facing dashboard server.
//!
//! This crate holds the surface a process needs to render or aggregate live
//! terminals without linking the PTY engine: the [`TerminalFrame`] /
//! [`ProducerSnapshot`] wire types, the discovery paths ([`socket_dir`],
//! [`socket_path`]), and the read-only web [`Dashboard`] (the [`Hub`] Loro
//! document plus its SSE server, [`serve_hub`]).
//!
//! The `tui` crate owns the PTY engine and re-exports these names, building the
//! frames from a live manager; the standalone aggregator (`tui-dashboard`)
//! depends on this crate alone so it never links the native VT library.

#![allow(
    clippy::significant_drop_tightening,
    reason = "guard-then-extract is the natural read pattern for the shared CRDT and broadcast locks"
)]
#![allow(
    clippy::struct_excessive_bools,
    reason = "terminal cell attributes are intrinsically parallel booleans"
)]

mod dashboard;
mod error;
mod frame;

pub use dashboard::{Dashboard, Hub, serve_hub};
pub use error::{Error, Result};
pub use frame::{ProducerSnapshot, TerminalFrame, socket_dir, socket_path};
