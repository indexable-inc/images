//! Wire types and discovery paths shared by the producer ([`crate::publish`])
//! and the dashboard/aggregator ([`crate::dashboard`]).
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
    /// The visible screen, rows newline-joined.
    pub screen: String,
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

/// Sample every terminal the manager tracks into a frame list.
///
/// A terminal whose read fails this tick is skipped, not dropped from the set:
/// the next tick re-reads it. Shared by the in-process dashboard poller and the
/// producer so both render the same snapshot of a manager.
pub async fn collect_frames(manager: &crate::TuiManager) -> Vec<TerminalFrame> {
    let mut frames = Vec::new();
    for instance in manager.list() {
        let Ok(full) = instance.read_full_async().await else {
            continue;
        };
        frames.push(TerminalFrame {
            id: instance.id.to_string(),
            command: instance.command.clone(),
            args: instance.args.join(" "),
            rows: instance.rows(),
            cols: instance.cols(),
            alive: instance.is_alive(),
            screen: full.viewport.join("\n"),
        });
    }
    frames
}
