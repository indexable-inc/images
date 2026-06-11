//! Wire protocol and runtime paths for [`tap`](https://github.com/indexable-inc/index)
//! terminal sessions.
//!
//! A `tap` client and the session daemon speak newline-delimited JSON over a
//! Unix domain socket: one [`Request`] or [`Response`] object per line. Binary
//! payloads (raw PTY bytes, resync snapshots) ride as base64 strings so a frame
//! never contains a literal newline and the stream stays line-splittable.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Environment variable that overrides the directory holding session sockets and
/// the session index. Set it to isolate state (tests use this).
pub const RUNTIME_DIR_ENV: &str = "TAP_RUNTIME_DIR";

/// Session metadata persisted in the session index file.
///
/// `started_unix` is seconds since the Unix epoch, recorded by the daemon at
/// spawn. Readers resolve liveness from `pid` and `socket` rather than trusting
/// this record, so a crashed daemon's stale entry is ignored, not shown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Human-readable session id, also the socket file stem.
    pub id: String,
    /// PID of the owning daemon process.
    pub pid: u32,
    /// Seconds since the Unix epoch when the session started.
    pub started_unix: u64,
    /// The command (argv) the session runs.
    pub command: Vec<String>,
    /// Absolute path to the session's Unix socket.
    pub socket: PathBuf,
}

/// A request from a client to the session daemon. One JSON object per line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Take over the session as an interactive client of the given terminal
    /// size. The connection then streams [`Response`] events until detach.
    Attach {
        /// Client terminal rows.
        rows: u16,
        /// Client terminal columns.
        cols: u16,
    },
    /// Leave an attached session, keeping the daemon and child alive.
    Detach,
    /// Forward keystrokes from an attached client to the child.
    Input {
        /// Raw input bytes (base64 on the wire).
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    /// Report a new client terminal size (drives min-size negotiation).
    Resize {
        /// New client terminal rows.
        rows: u16,
        /// New client terminal columns.
        cols: u16,
    },
    /// Write text into the child without attaching (scripting hook).
    Inject {
        /// UTF-8 text to feed to the child.
        data: String,
    },
    /// Stream raw output without contributing to size negotiation or input.
    Subscribe,
    /// Read the current screen as plain text, optionally the last `lines` rows.
    GetScrollback {
        /// Limit to the last N rows, or the whole screen when `None`.
        lines: Option<usize>,
    },
    /// Read the current cursor position.
    GetCursor,
    /// Read the negotiated session size.
    GetSize,
    /// Terminate the session: kill the child and shut the daemon down.
    Kill,
}

/// A response or event from the daemon to a client. One JSON object per line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Attach accepted. `snapshot` reproduces the current screen (colors and
    /// cursor) so the freshly attached terminal paints correctly; `rows`/`cols`
    /// are the negotiated session size.
    Attached {
        /// Negotiated session rows.
        rows: u16,
        /// Negotiated session columns.
        cols: u16,
        /// Escape-sequence snapshot of the current screen (base64 on the wire).
        #[serde(with = "b64")]
        snapshot: Vec<u8>,
    },
    /// Live child output (base64 on the wire).
    Output {
        /// Raw output bytes.
        #[serde(with = "b64")]
        data: Vec<u8>,
    },
    /// The negotiated session size changed. `snapshot` is a fresh repaint of the
    /// resized screen so every client converges without waiting for the child.
    Resized {
        /// New negotiated session rows.
        rows: u16,
        /// New negotiated session columns.
        cols: u16,
        /// Escape-sequence snapshot of the resized screen (base64 on the wire).
        #[serde(with = "b64")]
        snapshot: Vec<u8>,
    },
    /// Plain-text screen contents.
    Scrollback {
        /// The requested screen text.
        content: String,
    },
    /// Cursor position (0-indexed row, col).
    Cursor {
        /// Cursor row.
        row: u16,
        /// Cursor column.
        col: u16,
    },
    /// Negotiated session size.
    Size {
        /// Session rows.
        rows: u16,
        /// Session columns.
        cols: u16,
    },
    /// Subscription confirmed; `Output` events follow.
    Subscribed,
    /// The child exited; the daemon is shutting the session down.
    SessionEnded {
        /// Resolved child exit code (128 + signal when signalled).
        exit_code: i32,
    },
    /// A control request succeeded.
    Ok,
    /// A request failed.
    Error {
        /// Operator-facing message.
        message: String,
    },
}

/// Directory holding session sockets and the session index.
///
/// Honors [`RUNTIME_DIR_ENV`], then the XDG runtime dir, then `~/.tap`, then a
/// temp-dir fallback so the path resolves even without a session bus.
#[must_use]
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(RUNTIME_DIR_ENV) {
        return PathBuf::from(dir);
    }
    dirs::runtime_dir()
        .map(|d| d.join("tap"))
        .or_else(|| dirs::home_dir().map(|h| h.join(".tap")))
        .unwrap_or_else(|| std::env::temp_dir().join("tap"))
}

/// Socket path for a session id.
#[must_use]
pub fn socket_path(session_id: &str) -> PathBuf {
    runtime_dir().join(format!("{session_id}.sock"))
}

/// Path to the JSON session index.
#[must_use]
pub fn sessions_file() -> PathBuf {
    runtime_dir().join("sessions.json")
}

/// Serde adapter that stores a byte vector as a base64 string on the wire.
mod b64 {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        STANDARD
            .decode(encoded.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_round_trips_arbitrary_bytes_through_base64() {
        let original = Response::Output {
            data: vec![0x00, 0x1b, b'[', b'3', b'1', b'm', 0xff, b'\n'],
        };
        let line = serde_json::to_string(&original).unwrap();

        // A frame must stay on one line so newline-splitting the stream is safe.
        assert!(!line.trim_end().contains('\n'));

        let decoded: Response = serde_json::from_str(&line).unwrap();
        match decoded {
            Response::Output { data } => {
                assert_eq!(data, vec![0x00, 0x1b, b'[', b'3', b'1', b'm', 0xff, b'\n']);
            }
            other => panic!("expected Output, got {other:?}"),
        }
    }

    #[test]
    fn runtime_dir_honors_env_override() {
        // Safe in a single-threaded unit test; documents the test-isolation hook.
        unsafe { std::env::set_var(RUNTIME_DIR_ENV, "/tmp/tap-test-xyz") };
        assert_eq!(runtime_dir(), PathBuf::from("/tmp/tap-test-xyz"));
        assert_eq!(
            socket_path("s1"),
            PathBuf::from("/tmp/tap-test-xyz/s1.sock")
        );
        unsafe { std::env::remove_var(RUNTIME_DIR_ENV) };
    }
}
