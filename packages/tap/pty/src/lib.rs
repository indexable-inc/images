//! Multiplexable PTY session engine.
//!
//! [`PtySession::spawn`] runs a child on a real PTY and mirrors its output into
//! a [`vt100`] emulator. Any number of subscribers can [`subscribe`] to the live
//! raw byte stream, and each subscription starts with a `snapshot`: the escape
//! sequences that reproduce the current screen (colors, attributes, cursor) so a
//! freshly attached terminal paints the right thing instead of a blank rectangle.
//!
//! The snapshot and the live stream are handed out atomically: the actor holds
//! the emulator lock across both the `process` of new bytes and their broadcast,
//! so a subscriber that snapshots under the same lock can never miss or duplicate
//! a byte across the join. That property is what makes late attach correct, which
//! is the bug a session manager has to get right.
//!
//! [`subscribe`]: PtySession::subscribe

use std::sync::Arc;

use bytes::Bytes;
use parking_lot::Mutex;
use snafu::{OptionExt as _, ResultExt as _, Snafu};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::{broadcast, mpsc, watch};

/// How many output chunks the broadcast buffers before a slow subscriber lags.
/// A lagged subscriber resyncs from a fresh snapshot, so this only bounds memory.
const OUTPUT_CHANNEL_CAP: usize = 1024;

/// PTY read buffer size, matching the engine in `packages/tui`.
const READ_BUFFER_SIZE: usize = 8192;

/// Errors from the PTY session engine.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    /// The command (argv) was empty, so there is no program to run.
    #[snafu(display("cannot spawn a session with an empty command"))]
    EmptyCommand,
    /// Opening the PTY master failed.
    #[snafu(display("failed to open PTY: {source}"))]
    OpenPty {
        /// Underlying OS error.
        source: std::io::Error,
    },
    /// Sizing the PTY failed.
    #[snafu(display("failed to size PTY: {source}"))]
    Resize {
        /// Underlying OS error.
        source: std::io::Error,
    },
    /// Spawning the child process failed.
    #[snafu(display("failed to spawn `{program}`: {source}"))]
    Spawn {
        /// The program that failed to start.
        program: String,
        /// Underlying OS error.
        source: std::io::Error,
    },
    /// The session actor has stopped, so the request cannot be served.
    #[snafu(display("session has stopped"))]
    Stopped,
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Parameters for [`PtySession::spawn`].
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Command to run as argv; `command[0]` is the program.
    pub command: Vec<String>,
    /// Initial terminal rows.
    pub rows: u16,
    /// Initial terminal columns.
    pub cols: u16,
    /// Lines of scrollback the emulator retains.
    pub scrollback_lines: usize,
}

/// Terminal dimensions in rows and columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinSize {
    /// Number of rows.
    pub rows: u16,
    /// Number of columns.
    pub cols: u16,
}

/// A 0-indexed cursor position on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPosition {
    /// 0-indexed row.
    pub row: u16,
    /// 0-indexed column.
    pub col: u16,
}

/// A live attachment: a resync `snapshot` plus the raw output stream.
///
/// The snapshot reproduces the screen at the moment of attach; `output` carries
/// every byte the child emits after that, with no gap and no overlap.
pub struct Attachment {
    /// Escape sequences reproducing the current screen.
    pub snapshot: Vec<u8>,
    /// Live raw output following the snapshot.
    pub output: broadcast::Receiver<Bytes>,
}

/// Commands sent from a [`PtySession`] handle to its owning actor task. Only the
/// actor touches the PTY fd, so writes and PTY resizes serialize through here.
enum Command {
    Write(Vec<u8>),
    ResizePty { rows: u16, cols: u16 },
    Kill,
}

/// A running PTY-backed session. Clone-free handle whose methods talk to the
/// actor; cheap reads (screen, cursor, size) go straight to the shared emulator.
pub struct PtySession {
    emulator: Arc<Mutex<vt100::Parser>>,
    output_tx: broadcast::Sender<Bytes>,
    commands: mpsc::UnboundedSender<Command>,
    exit_rx: watch::Receiver<Option<i32>>,
}

impl PtySession {
    /// Spawn `config.command` on a fresh PTY sized `rows`×`cols`.
    ///
    /// Must be called from within a Tokio runtime: it starts the actor task and
    /// creates a [`tokio::process::Child`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the command is empty or the PTY/child cannot be
    /// created.
    pub fn spawn(config: SessionConfig) -> Result<Self> {
        let SessionConfig {
            command,
            rows,
            cols,
            scrollback_lines,
        } = config;
        let (program, args) = command.split_first().context(EmptyCommandSnafu)?;

        // `pty_process::open` allocates the PTY master and its slave together.
        let (pty, pts) = pty_process::open()
            .map_err(std::io::Error::other)
            .context(OpenPtySnafu)?;
        pty.resize(pty_process::Size::new(rows, cols))
            .map_err(std::io::Error::other)
            .context(ResizeSnafu)?;
        let child = pty_process::Command::new(program)
            .args(args)
            .spawn(pts)
            .map_err(std::io::Error::other)
            .context(SpawnSnafu {
                program: program.clone(),
            })?;

        let emulator = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, scrollback_lines)));
        let (output_tx, _) = broadcast::channel(OUTPUT_CHANNEL_CAP);
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (exit_tx, exit_rx) = watch::channel(None);

        let actor_emulator = Arc::clone(&emulator);
        let actor_output = output_tx.clone();
        tokio::spawn(async move {
            actor(
                pty,
                child,
                actor_emulator,
                actor_output,
                command_rx,
                exit_tx,
            )
            .await;
        });

        Ok(Self {
            emulator,
            output_tx,
            commands,
            exit_rx,
        })
    }

    /// Start a live attachment: the current screen as a resync snapshot plus the
    /// raw output stream that follows it with no gap.
    #[must_use]
    pub fn subscribe(&self) -> Attachment {
        // Snapshot and subscribe under the emulator lock the actor also holds
        // across process+broadcast, so the join is exactly once: no byte is both
        // in the snapshot and the first stream item, and none is dropped between.
        let emulator = self.emulator.lock();
        let snapshot = emulator.screen().contents_formatted();
        let output = self.output_tx.subscribe();
        drop(emulator);
        Attachment { snapshot, output }
    }

    /// Escape sequences reproducing the current screen.
    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        self.emulator.lock().screen().contents_formatted()
    }

    /// Forward raw input bytes to the child.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Stopped`] if the session actor has exited.
    pub fn write_input(&self, data: Vec<u8>) -> Result<()> {
        self.commands
            .send(Command::Write(data))
            .map_err(|_| StoppedSnafu.build())
    }

    /// Resize the session. Updates the emulator immediately so a snapshot taken
    /// right after reflects the new geometry, and queues the kernel PTY resize
    /// (which delivers `SIGWINCH` to the child) on the actor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Stopped`] if the session actor has exited.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.emulator.lock().screen_mut().set_size(rows, cols);
        self.commands
            .send(Command::ResizePty { rows, cols })
            .map_err(|_| StoppedSnafu.build())
    }

    /// Terminate the child (SIGKILL). The session shuts down once it is reaped.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Stopped`] if the session actor has exited.
    pub fn kill(&self) -> Result<()> {
        self.commands
            .send(Command::Kill)
            .map_err(|_| StoppedSnafu.build())
    }

    /// Current screen as plain text, optionally only the last `lines` rows.
    #[must_use]
    pub fn scrollback(&self, lines: Option<usize>) -> String {
        let emulator = self.emulator.lock();
        let contents = emulator.screen().contents();
        drop(emulator);
        match lines {
            Some(n) => {
                let rows: Vec<&str> = contents.lines().collect();
                let start = rows.len().saturating_sub(n);
                rows.into_iter().skip(start).collect::<Vec<_>>().join("\n")
            }
            None => contents,
        }
    }

    /// Cursor position, both row and column 0-indexed.
    #[must_use]
    pub fn cursor(&self) -> CursorPosition {
        let (row, col) = self.emulator.lock().screen().cursor_position();
        CursorPosition { row, col }
    }

    /// Current emulator size.
    #[must_use]
    pub fn size(&self) -> WinSize {
        let (rows, cols) = self.emulator.lock().screen().size();
        WinSize { rows, cols }
    }

    /// Whether the child has switched to the alternate screen (full-screen TUI).
    #[must_use]
    pub fn alternate_screen(&self) -> bool {
        self.emulator.lock().screen().alternate_screen()
    }

    /// A watch receiver that becomes `Some(exit_code)` when the child exits.
    #[must_use]
    pub fn exit_watch(&self) -> watch::Receiver<Option<i32>> {
        self.exit_rx.clone()
    }

    /// The child's resolved exit code, or `None` while it is still running.
    #[must_use]
    pub fn exit_code(&self) -> Option<i32> {
        *self.exit_rx.borrow()
    }
}

/// Resolve a child's wait status into a conventional exit code: the raw code, or
/// `128 + signal` when it was signalled, or `-1` when the status is unavailable.
fn resolve_exit_code(status: std::io::Result<std::process::ExitStatus>) -> i32 {
    use std::os::unix::process::ExitStatusExt as _;
    status.map_or(-1, |status| {
        status
            .code()
            .or_else(|| status.signal().map(|signal| 128 + signal))
            .unwrap_or(-1)
    })
}

/// The single task that owns the PTY master and the child. Every PTY read, write,
/// resize, and the child reap serialize here, which keeps the emulator mirror and
/// the broadcast stream consistent for all subscribers.
async fn actor(
    mut pty: pty_process::Pty,
    mut child: tokio::process::Child,
    emulator: Arc<Mutex<vt100::Parser>>,
    output_tx: broadcast::Sender<Bytes>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    exit_tx: watch::Sender<Option<i32>>,
) {
    let mut read_buffer = [0u8; READ_BUFFER_SIZE];
    let mut pty_active = true;
    let mut child_exited = false;
    let mut commands_open = true;

    loop {
        tokio::select! {
            biased;

            command = commands.recv(), if commands_open => match command {
                Some(Command::Write(data)) => if pty_active {
                    let _ = pty.write_all(&data).await;
                },
                Some(Command::ResizePty { rows, cols }) => if pty_active {
                    let _ = pty.resize(pty_process::Size::new(rows, cols));
                },
                Some(Command::Kill) => {
                    // Harmless no-op if already reaped, so a redundant kill is fine.
                    let _ = child.start_kill();
                },
                None => commands_open = false,
            },

            result = pty.read(&mut read_buffer), if pty_active => match result {
                Ok(0) | Err(_) => pty_active = false,
                Ok(n) => {
                    let chunk = Bytes::copy_from_slice(&read_buffer[..n]);
                    // Hold the emulator lock across process+broadcast so a
                    // concurrent subscribe() snapshots a consistent screen and
                    // joins the stream exactly once. send() is non-blocking.
                    let mut guard = emulator.lock();
                    guard.process(&chunk);
                    let _ = output_tx.send(chunk);
                    drop(guard);
                }
            },

            status = child.wait(), if !child_exited => {
                child_exited = true;
                let _ = exit_tx.send(Some(resolve_exit_code(status)));
            },

            else => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    async fn wait_until<F: Fn() -> bool>(predicate: F) -> bool {
        for _ in 0..200 {
            if predicate() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        predicate()
    }

    #[tokio::test]
    async fn mirrors_child_output_into_the_screen() {
        let session = PtySession::spawn(SessionConfig {
            command: vec!["sh".into(), "-c".into(), "printf 'hello from pty'".into()],
            rows: 24,
            cols: 80,
            scrollback_lines: 1000,
        })
        .expect("spawn session");

        assert!(
            wait_until(|| session.scrollback(None).contains("hello from pty")).await,
            "screen never showed child output: {:?}",
            session.scrollback(None)
        );
    }

    #[tokio::test]
    async fn late_subscriber_gets_a_snapshot_of_prior_output() {
        let session = PtySession::spawn(SessionConfig {
            command: vec![
                "sh".into(),
                "-c".into(),
                "printf 'before attach'; sleep 5".into(),
            ],
            rows: 24,
            cols: 80,
            scrollback_lines: 1000,
        })
        .expect("spawn session");

        assert!(wait_until(|| session.scrollback(None).contains("before attach")).await);

        // A subscriber that joins after the output was produced still sees it,
        // because subscribe() carries a snapshot of the current screen.
        let attachment = session.subscribe();
        let snapshot = String::from_utf8_lossy(&attachment.snapshot);
        assert!(
            snapshot.contains("before attach"),
            "snapshot missing prior output: {snapshot:?}"
        );
    }

    #[tokio::test]
    async fn reports_child_exit_code() {
        let session = PtySession::spawn(SessionConfig {
            command: vec!["sh".into(), "-c".into(), "exit 7".into()],
            rows: 24,
            cols: 80,
            scrollback_lines: 100,
        })
        .expect("spawn session");

        let mut exit = session.exit_watch();
        let code = tokio::time::timeout(Duration::from_secs(5), async {
            exit.wait_for(Option::is_some).await.map(|v| v.unwrap())
        })
        .await
        .expect("child exited in time")
        .expect("exit watch open");

        assert_eq!(code, 7);
    }
}
