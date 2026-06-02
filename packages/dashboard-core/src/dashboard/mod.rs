//! A read-only web canvas for live MCP resources.
//!
//! The dashboard renders whatever panes sit in a [`Hub`]'s Loro document and
//! streams changes to browsers over Server-Sent Events. Two frame sources drive
//! a hub:
//!
//! * the in-process dashboard (`tui::serve`) polls one `TuiManager` and applies
//!   its terminals as panes under one scope.
//! * the standalone aggregator (`dashboard`) reads many producer sockets and
//!   applies each producer's panes under its own scope.
//!
//! Both paths share [`serve_hub`], the router, the page, and the SSE stream, so
//! there is one owner for the browser-facing surface. Loro is only the view-sync
//! layer: the resources stay in their owning process, browsers never write back,
//! so the doc has a single editor per scope and conflict resolution never runs;
//! the CRDT buys cheap incremental diffs, a late joiner catching up from one
//! snapshot, and, because the oplog is a timestamped recording, replay of any
//! pane's history for free. A [`RecordingStore`] persists that recording to disk
//! so a session survives a restart and can be opened or shared after the fact.

mod hub;
mod recordings;
mod server;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

pub use hub::Hub;
pub use recordings::{RecordingInfo, RecordingStore};
pub use server::{Dashboard, serve_hub};

/// Base64 for the SSE wire. One spelling shared by the snapshot and update
/// encoders in [`server`].
fn b64(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}
