//! Engine-free wire types, a generic pane publisher, and the browser-facing
//! canvas server.
//!
//! This crate holds the surface a process needs to render or aggregate live
//! resources without linking any engine: the [`Pane`] / [`View`] /
//! [`ProducerSnapshot`] wire types, the discovery paths ([`discovery_dir`],
//! [`socket_path`]), the lightweight [`Publisher`], and the read-only web
//! [`Dashboard`] (the [`Hub`] Loro document plus its SSE server, [`serve_hub`]).
//!
//! A pane is a titled card whose body is one [`View`]: a bandwidth-cheap
//! [`TerminalView`] with a built-in renderer, a producer-defined [`HtmlView`],
//! or a [`DataView`] of JSON rendered by a named frontend renderer. Any producer
//! builds a `Pane` list and streams it through [`Publisher`]; the aggregator
//! folds every producer into one document and serves the canvas. The `tui` crate
//! re-exports these names and adapts its PTY manager into terminal panes; the
//! standalone aggregator (`dashboard`) depends on this crate alone so it never
//! links a native engine.

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
mod pane;
mod publish;

pub use dashboard::{Dashboard, Hub, RecordingInfo, RecordingStore, serve_hub};
pub use error::{Error, Result};
pub use pane::{
    DataView, ExecTraceLine, ExecView, HtmlView, Pane, ProducerSnapshot, TerminalView, View,
    discovery_dir, socket_path,
};
pub use publish::{PaneSink, Publisher};
