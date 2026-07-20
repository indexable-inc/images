//! The per-session VT engine: a dedicated OS thread that owns the `!Send`
//! [`ix_vt::Terminal`] (libghostty-vt) and turns byte feeds into dirty-row
//! grid frames on the session's broadcast channel.
//!
//! libghostty-vt's terminal has thread affinity, so one pinned thread owns it
//! (the same shape as `packages/tui/tui`'s engine actor). Frames are
//! coalesced: under a redraw storm the engine renders at most one frame per
//! [`FRAME_INTERVAL`], diffing whole snapshots rather than tracking ghostty's
//! internal dirty flags, which keeps the wire format independent of the
//! engine (index#3797 wants it swappable).

use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;

use crate::proto::{ServerMsg, cursor_wire, diff_snapshots};

/// Ceiling on the frame rate under continuous output (~30 fps). A quiet
/// stream renders as soon as it goes idle, so interactive latency is the
/// engine's poll granularity, not this interval.
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// How long the engine sleeps when there is nothing dirty to ship.
const IDLE_WAIT: Duration = Duration::from_secs(1);

/// A request to the engine thread.
pub enum EngineMsg {
    /// Feed raw VT bytes (already stripped of OSC 5522 by the scanner).
    Feed(Vec<u8>),
    /// Resize the terminal; the next frame is full.
    Resize {
        /// New height in rows.
        rows: u16,
        /// New width in columns.
        cols: u16,
    },
    /// Ship a full frame (a client connected or resynced after lag).
    FullFrame,
    /// Stop the engine thread.
    Shutdown,
}

/// A handle to a session's engine thread.
#[derive(Clone)]
pub struct EngineHandle {
    tx: Sender<EngineMsg>,
}

impl EngineHandle {
    /// Send a request; a dead engine (session tear-down) drops it.
    pub fn send(&self, msg: EngineMsg) {
        if self.tx.send(msg).is_err() {
            tracing::debug!("engine request after engine shutdown");
        }
    }
}

/// Spawn the engine thread for a `rows`x`cols` terminal.
///
/// Grid frames (and nothing else) are published to `events`. The terminal is
/// created on the new thread; creation failure surfaces here through an init
/// handshake instead of leaving a dead channel.
///
/// # Errors
/// Returns an error if the OS thread cannot be spawned or libghostty-vt
/// cannot allocate the terminal.
pub fn spawn_engine(
    rows: u16,
    cols: u16,
    scrollback: usize,
    events: broadcast::Sender<Arc<ServerMsg>>,
) -> anyhow::Result<EngineHandle> {
    let (tx, rx) = std::sync::mpsc::channel::<EngineMsg>();
    let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<(), ix_vt::Error>>(1);

    std::thread::Builder::new()
        .name("ix-term-vt".to_owned())
        .spawn(move || {
            let mut terminal = match ix_vt::Terminal::new(rows, cols, scrollback) {
                Ok(terminal) => {
                    if init_tx.send(Ok(())).is_err() {
                        return;
                    }
                    terminal
                }
                Err(error) => {
                    let _ = init_tx.send(Err(error));
                    return;
                }
            };
            engine_loop(&mut terminal, &rx, &events);
        })?;

    init_rx.recv()??;
    Ok(EngineHandle { tx })
}

/// The engine thread body: drain requests, pace frames.
fn engine_loop(
    terminal: &mut ix_vt::Terminal,
    rx: &std::sync::mpsc::Receiver<EngineMsg>,
    events: &broadcast::Sender<Arc<ServerMsg>>,
) {
    let mut prev: Option<ix_vt::Snapshot> = None;
    let mut seq: u64 = 0;
    let mut dirty = false;
    let mut last_frame = Instant::now()
        .checked_sub(FRAME_INTERVAL)
        .unwrap_or_else(Instant::now);

    loop {
        // Sleep until the next frame is due (or idle-poll when clean);
        // requests wake the loop early.
        let wait = if dirty {
            FRAME_INTERVAL.saturating_sub(last_frame.elapsed())
        } else {
            IDLE_WAIT
        };
        match rx.recv_timeout(wait) {
            Ok(msg) => {
                if handle(terminal, msg, &mut prev, &mut dirty).is_break() {
                    return;
                }
                // Drain whatever queued behind it so a burst becomes one frame.
                loop {
                    match rx.try_recv() {
                        Ok(msg) => {
                            if handle(terminal, msg, &mut prev, &mut dirty).is_break() {
                                return;
                            }
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        if dirty && last_frame.elapsed() >= FRAME_INTERVAL {
            render_frame(terminal, &mut prev, &mut seq, events);
            dirty = false;
            last_frame = Instant::now();
        }
    }
}

/// Apply one request to the terminal. `Break` means shut down.
fn handle(
    terminal: &mut ix_vt::Terminal,
    msg: EngineMsg,
    prev: &mut Option<ix_vt::Snapshot>,
    dirty: &mut bool,
) -> std::ops::ControlFlow<()> {
    match msg {
        EngineMsg::Feed(bytes) => {
            terminal.vt_write(&bytes);
            *dirty = true;
        }
        EngineMsg::Resize { rows, cols } => {
            if let Err(error) = terminal.resize(rows, cols) {
                tracing::warn!(%error, rows, cols, "terminal resize rejected");
            }
            *prev = None;
            *dirty = true;
        }
        EngineMsg::FullFrame => {
            *prev = None;
            *dirty = true;
        }
        EngineMsg::Shutdown => return std::ops::ControlFlow::Break(()),
    }
    std::ops::ControlFlow::Continue(())
}

/// Render, diff against the previous snapshot, and broadcast a grid frame.
fn render_frame(
    terminal: &ix_vt::Terminal,
    prev: &mut Option<ix_vt::Snapshot>,
    seq: &mut u64,
    events: &broadcast::Sender<Arc<ServerMsg>>,
) {
    let snapshot = match terminal.render() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::warn!(%error, "terminal render failed; skipping frame");
            return;
        }
    };
    if prev.as_ref() == Some(&snapshot) {
        return;
    }
    let diff = diff_snapshots(prev.as_ref(), &snapshot);
    let app_cursor = terminal.application_cursor_keys().unwrap_or(false);
    let msg = ServerMsg::Grid {
        seq: *seq,
        cols: snapshot.cols,
        rows: snapshot.rows,
        full: diff.full,
        changed: diff.changed,
        cursor: cursor_wire(&snapshot.cursor),
        app_cursor,
    };
    *seq += 1;
    *prev = Some(snapshot);
    // No receivers is fine: frames before the first client attach are simply
    // state the next full frame covers.
    let _ = events.send(Arc::new(msg));
}
