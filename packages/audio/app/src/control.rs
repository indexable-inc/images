//! The daemon's control-socket protocol: JSON lines over a unix socket.
//!
//! One serde-rendered shape for every client (CLI, macOS tray, the MCP
//! kernel's python module); nothing hand-assembles these strings.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Overrides the control socket path (and where the daemon binds it).
pub const SOCKET_ENV: &str = "SHARED_AUDIO_SOCKET";

/// Daemon state directory: score snapshot, blob store, control socket.
#[must_use]
pub fn state_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .expect("a state or local-data directory exists")
        .join("shared-audio")
}

/// Where the control socket lives: `$SHARED_AUDIO_SOCKET` or the state dir.
#[must_use]
pub fn socket_path() -> PathBuf {
    socket_path_in(&state_dir())
}

/// The control socket under a specific state directory (a `--state-dir`
/// override); `$SHARED_AUDIO_SOCKET` still wins.
#[must_use]
pub fn socket_path_in(state_dir: &Path) -> PathBuf {
    std::env::var_os(SOCKET_ENV)
        .map_or_else(|| state_dir.join("control.sock"), PathBuf::from)
}

/// A single client request; one JSON object per line.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Report daemon, clock, and score state.
    Status,
    /// Adjust this listener's local volume. Never touches the score.
    Volume {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        set: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        muted: Option<bool>,
    },
    /// Publish a new WASM (or WAT) instrument to every peer.
    Publish {
        wasm_base64: String,
        /// Shared frame to switch at; omitted means "one second from now".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        at_frame: Option<u64>,
    },
    /// Set a shared control immediately.
    SetControl { control: u16, value: f32 },
    /// Schedule a shared control change at an exact shared frame.
    Schedule { at_frame: u64, control: u16, value: f32 },
}

/// The daemon's one-line JSON reply.
#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

impl Response {
    #[must_use]
    pub fn ok() -> Self {
        Self { ok: true, error: None, status: None }
    }

    #[must_use]
    pub fn err(error: impl std::fmt::Display) -> Self {
        Self { ok: false, error: Some(error.to_string()), status: None }
    }
}

/// A point-in-time view of the daemon for `status`.
#[derive(Debug, Serialize, Deserialize)]
pub struct Status {
    pub peer_id: u64,
    pub tcp_addr: String,
    pub udp_addr: String,
    pub sample_rate: u32,
    /// Shared-timeline frame at the moment of the request.
    pub frame_now: i64,
    pub epoch_micros: i64,
    pub gain: f32,
    pub muted: bool,
    /// Hex hash of the active instrument module, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument: Option<String>,
    pub controls: Vec<(u16, f32)>,
    pub events: usize,
}
