//! PTY-backed terminal management: spawn child processes attached to real
//! pseudo-terminals, drive them with input, and read back a VT-rendered
//! viewport, scrollback, and per-cell styling.
//!
//! [`TuiManager`] spawns processes and tracks them; each spawn returns a
//! [`TuiInstance`] handle that carries every read and write method. Blocking
//! methods drive the manager's shared tokio runtime; their `_async` twins
//! return futures for callers that already have one.
//!
//! ```no_run
//! use std::time::Duration;
//! use tui::{SpawnConfig, TuiManager};
//!
//! let manager = TuiManager::new();
//! let term = manager.spawn("cat".into(), vec![], SpawnConfig::default())?;
//! term.write("hello\n")?;
//! let lines = term.read_blocking(Duration::from_secs(1))?;
//! # Ok::<(), tui::Error>(())
//! ```

#![allow(
    clippy::missing_errors_doc,
    reason = "library API errors are documented via the typed `Error` enum"
)]
#![allow(
    clippy::significant_drop_tightening,
    reason = "guard-then-extract is the natural read pattern for the shared CRDT and cache locks"
)]
#![allow(
    clippy::struct_excessive_bools,
    reason = "terminal cell attributes are intrinsically four parallel booleans"
)]
#![allow(
    clippy::option_if_let_else,
    reason = "explicit match is clearer than map_or at the cell extraction sites"
)]

mod actor;
#[cfg(feature = "dashboard")]
pub mod dashboard;
mod error;
#[cfg(any(feature = "dashboard", feature = "publish"))]
mod frame;
mod manager;
#[cfg(feature = "publish")]
pub mod publish;
mod slice;
mod types;

#[cfg(feature = "dashboard")]
pub use dashboard::serve;
pub use error::{Error, Result};
#[cfg(any(feature = "dashboard", feature = "publish"))]
pub use dashboard_core::{Pane, ProducerSnapshot, TerminalView, View, discovery_dir, socket_path};
#[cfg(feature = "dashboard")]
pub use dashboard_core::{Dashboard, Hub, serve_hub};
pub use manager::{TuiInstance, TuiManager};
#[cfg(feature = "publish")]
pub use publish::{Publisher, publish};
pub use slice::{ColRange, RowRange, slice_2d};
pub use types::{Color, CursorPos, CursorShape, ExitState, FullOutput, SpawnConfig, StyledCell};

#[cfg(test)]
mod tests;
