//! Wire types and discovery paths shared by the producer (`tui::publish`) and
//! the dashboard/aggregator ([`crate::dashboard`]).
//!
//! A producer streams [`ProducerSnapshot`]s over a unix socket; the aggregator
//! folds them into one document keyed by `producer`. Both halves agree on these
//! shapes and on where the sockets live ([`socket_dir`]), so neither side
//! reaches into the other.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One terminal's rendered state at a single poll tick.
///
/// The unit a producer streams and the dashboard renders. `id` is unique within
/// its producer; the aggregator namespaces it by producer for a global key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalFrame {
    /// Stable per-producer terminal id (the manager's UUID).
    pub id: String,
    /// The command that was spawned.
    pub command: String,
    /// Positional arguments, space-joined for display.
    pub args: String,
    /// Terminal height in rows.
    pub rows: u16,
    /// Terminal width in columns.
    pub cols: u16,
    /// Whether the child is still running.
    pub alive: bool,
    /// The visible screen, rows newline-joined, with minimal ANSI SGR runs
    /// encoding per-cell color and attributes. The dashboard parses the SGR
    /// back into styled spans; a plain reader still sees the text.
    pub screen: String,
    // These fields were added after the first wire shape. `#[serde(default)]`
    // keeps a mixed-version dashboard working: a producer built before this
    // change streams frames without them, and the aggregator drops a frame
    // whose JSON fails to parse, so without defaults those older producers'
    // terminals would silently vanish from the dashboard.
    /// Cursor row in viewport cell coordinates (0-based, top first).
    #[serde(default)]
    pub cursor_row: u16,
    /// Cursor column in viewport cell coordinates (0-based, left first).
    #[serde(default)]
    pub cursor_col: u16,
    /// Whether the screen is showing its cursor (the inverse of `CSI ?25l`).
    #[serde(default)]
    pub cursor_visible: bool,
    /// The cursor shape token: `"block"`, `"underline"`, or `"bar"`.
    #[serde(default)]
    pub cursor_shape: String,
    /// The child's exit code when it has exited with one, else `None` (still
    /// running, or terminated by a signal).
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// One producer process's terminals, as sent over its discovery socket.
///
/// `producer` namespaces every terminal in `terminals` so many processes can
/// share one aggregated document without key collisions. Each message carries
/// the producer's full terminal set, so the latest message fully describes that
/// producer and a late-joining reader needs no backlog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProducerSnapshot {
    /// Stable per-process id: `"<pid>-<short-uuid>"`.
    pub producer: String,
    /// Every terminal this producer currently tracks.
    pub terminals: Vec<TerminalFrame>,
}

/// The directory where producers expose their per-process sockets and the
/// aggregator looks for them.
///
/// Resolved in order: `$IX_TUI_DIR`, then `$XDG_RUNTIME_DIR/ix-tui`, then
/// `/tmp/ix-tui-<user>`. Kept deliberately short: macOS caps a unix socket
/// path (`sun_path`) at 104 bytes, and `$TMPDIR` on macOS is long enough to
/// blow that budget once a filename is appended, so it is not used.
#[must_use]
pub fn socket_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("IX_TUI_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("ix-tui");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "shared".to_owned());
    PathBuf::from(format!("/tmp/ix-tui-{user}"))
}

/// A unique socket path for the current process inside [`socket_dir`].
///
/// The filename is `"<pid>-<short-uuid>.sock"`: the pid is human-legible for
/// debugging and the uuid suffix keeps it unique across pid reuse.
#[must_use]
pub fn socket_path() -> PathBuf {
    let short = uuid::Uuid::new_v4().simple().to_string();
    socket_dir().join(format!(
        "{}-{}.sock",
        std::process::id(),
        &short[..8]
    ))
}

#[cfg(test)]
mod tests {
    use super::TerminalFrame;

    /// A frame streamed by a producer built before the cursor/exit fields were
    /// added still deserializes: the new fields fall back to their defaults
    /// instead of failing the whole `ProducerSnapshot` parse and dropping the
    /// terminal from the dashboard.
    #[test]
    fn old_wire_shape_deserializes_with_field_defaults() {
        let old = r#"{
            "id": "t1", "command": "vim", "args": "-u NONE",
            "rows": 24, "cols": 80, "alive": true, "screen": "hi"
        }"#;
        let frame: TerminalFrame = serde_json::from_str(old).expect("old shape parses");
        assert_eq!(frame.screen, "hi");
        assert_eq!(frame.cursor_row, 0);
        assert_eq!(frame.cursor_col, 0);
        assert!(!frame.cursor_visible);
        assert_eq!(frame.cursor_shape, "");
        assert_eq!(frame.exit_code, None);
    }
}
