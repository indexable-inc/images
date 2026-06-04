//! The VT engine thread: a dedicated OS thread that owns the `!Send`
//! [`ix_vt::Terminal`] and serves the async actor over a channel.
//!
//! libghostty-vt's terminal has thread affinity (it is `!Send + !Sync`), so it
//! cannot live inside a tokio task that may move across worker threads. Instead
//! one pinned thread owns the terminal and the actor forwards every byte feed
//! and read request to it as an [`EngineRequest`]. Replies ride back on the
//! per-request oneshot channels, so the async side never touches the terminal.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, SyncSender};

use parking_lot::RwLock as SyncRwLock;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::Error;
use crate::error::Result;
use crate::types::{CursorShape, StyledCell};

/// A request to the VT engine thread.
///
/// Reads carry a oneshot reply sender; [`EngineRequest::Process`] has no reply
/// because byte feeds are fire-and-forget.
pub enum EngineRequest {
    /// Feed raw PTY bytes into the terminal.
    Process(Vec<u8>),
    /// Resize the terminal to `rows` x `cols`.
    Resize {
        rows: u16,
        cols: u16,
        reply: oneshot::Sender<Result<()>>,
    },
    /// Capture a render snapshot of the current viewport.
    Snapshot {
        reply: oneshot::Sender<Result<ix_vt::Snapshot>>,
    },
    /// Read the scrollback history above the viewport, oldest first.
    Scrollback {
        reply: oneshot::Sender<Result<Vec<String>>>,
    },
}

/// Map an `ix_vt` error into the crate's observable VT-engine error.
fn vt_engine_error(id: Uuid, source: ix_vt::Error) -> Error {
    Error::VtEngine {
        id,
        message: source.to_string(),
    }
}

/// Spawn the VT engine thread and return its request channel.
///
/// The terminal is created on the new thread (it cannot be moved there), so the
/// caller blocks on an init handshake: a failed `Terminal::new` surfaces here as
/// [`Error::VtEngine`] instead of silently leaving a dead thread. The returned
/// [`Sender`] is `Send`, so the async actor can move it into a tokio task.
///
/// # Errors
/// Returns [`Error::VtEngine`] if the engine thread cannot be spawned or if
/// `ix_vt::Terminal::new` fails on it.
pub fn spawn(
    id: Uuid,
    rows: u16,
    cols: u16,
    scrollback: usize,
    cursor_shape: Arc<SyncRwLock<CursorShape>>,
    app_cursor_keys: Arc<SyncRwLock<bool>>,
) -> Result<Sender<EngineRequest>> {
    let (tx, rx) = std::sync::mpsc::channel::<EngineRequest>();
    let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

    std::thread::Builder::new()
        .name("ix-vt-engine".to_owned())
        .spawn(move || {
            engine_loop(
                EngineConfig {
                    id,
                    rows,
                    cols,
                    scrollback,
                },
                &cursor_shape,
                &app_cursor_keys,
                &rx,
                &init_tx,
            );
        })
        .map_err(|e| vt_engine_error_io(id, &e))?;

    // Propagate the terminal-creation result before handing back the channel, so
    // a failed init is observable rather than a channel into a dead thread.
    init_rx
        .recv()
        .map_err(|e| Error::VtEngine {
            id,
            message: format!("VT engine thread exited before init: {e}"),
        })??;

    Ok(tx)
}

/// Map a thread-spawn io error into the crate's VT-engine error.
fn vt_engine_error_io(id: Uuid, source: &std::io::Error) -> Error {
    Error::VtEngine {
        id,
        message: format!("failed to spawn VT engine thread: {source}"),
    }
}

/// Terminal-construction parameters threaded from [`spawn`] into the engine
/// thread. Grouped so `engine_loop` stays under clippy's argument-count limit
/// and so the values that only describe the terminal travel as one unit.
/// `Copy` because every field is `Copy`: passing the struct by value otherwise
/// trips `clippy::needless_pass_by_value`.
#[derive(Clone, Copy)]
struct EngineConfig {
    id: Uuid,
    rows: u16,
    cols: u16,
    scrollback: usize,
}

/// The engine thread body: create the terminal, report init, then serve
/// requests until the channel closes.
fn engine_loop(
    config: EngineConfig,
    cursor_shape: &Arc<SyncRwLock<CursorShape>>,
    app_cursor_keys: &Arc<SyncRwLock<bool>>,
    rx: &Receiver<EngineRequest>,
    init_tx: &SyncSender<Result<()>>,
) {
    let EngineConfig {
        id,
        rows,
        cols,
        scrollback,
    } = config;
    let mut terminal = match ix_vt::Terminal::new(rows, cols, scrollback) {
        Ok(terminal) => terminal,
        Err(e) => {
            let _ = init_tx.send(Err(vt_engine_error(id, e)));
            return;
        }
    };
    let _ = init_tx.send(Ok(()));

    while let Ok(request) = rx.recv() {
        match request {
            EngineRequest::Process(bytes) => {
                terminal.vt_write(&bytes);
                // Refresh the cached cursor-key mode so the actor picks the
                // right arrow-key form on the next write. The mode is set by
                // the program's own output (terminfo `smkx`/`rmkx`), so reading
                // it here, right after feeding that output, keeps it current. A
                // failed query leaves the last known value: the mode did not
                // change, we just could not re-read it.
                if let Ok(app) = terminal.application_cursor_keys() {
                    *app_cursor_keys.write() = app;
                }
            }
            EngineRequest::Resize { rows, cols, reply } => {
                let result = terminal.resize(rows, cols).map_err(|e| vt_engine_error(id, e));
                let _ = reply.send(result);
            }
            EngineRequest::Snapshot { reply } => {
                let rendered = terminal.render();
                if let Ok(ref snapshot) = rendered {
                    *cursor_shape.write() = CursorShape::from(snapshot.cursor.visual_style);
                }
                let _ = reply.send(rendered.map_err(|e| vt_engine_error(id, e)));
            }
            EngineRequest::Scrollback { reply } => {
                let _ = reply.send(read_scrollback(id, &mut terminal));
            }
        }
    }
}

/// Read the full scrollback history, oldest line first.
///
/// `render` always reads the active viewport, so this walks the viewport up to
/// the oldest scrollback row and renders one row at a time, then restores the
/// active viewport. It mirrors the row-by-row read the old vt100 path did via
/// `set_scrollback`.
fn read_scrollback(id: Uuid, terminal: &mut ix_vt::Terminal) -> Result<Vec<String>> {
    let snapshot = terminal.render().map_err(|e| vt_engine_error(id, e))?;
    let total = snapshot.scrollback;
    if total == 0 {
        return Ok(Vec::new());
    }

    terminal.scroll_viewport(ix_vt::ScrollViewport::Top);
    let mut lines = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
    for i in 0..total {
        if i > 0 {
            terminal.scroll_viewport(ix_vt::ScrollViewport::Delta(1));
        }
        let page = terminal.render().map_err(|e| vt_engine_error(id, e))?;
        if let Some(first) = page.viewport.first() {
            lines.push(row_to_string(first));
        }
    }
    terminal.scroll_viewport(ix_vt::ScrollViewport::Bottom);

    Ok(lines)
}

/// A snapshot row joined into a string, trailing blanks trimmed.
fn row_to_string(row: &[ix_vt::Cell]) -> String {
    let mut line: String = row.iter().map(|cell| cell.ch.unwrap_or(' ')).collect();
    line.truncate(line.trim_end().len());
    line
}

/// The viewport's lines, top first, with trailing fully-blank rows dropped.
///
/// An all-blank screen yields an empty `Vec`, matching the old vt100
/// `screen().contents().lines()` behavior that callers poll on for first paint.
pub fn snapshot_to_viewport_lines(snapshot: &ix_vt::Snapshot) -> Vec<String> {
    let mut lines: Vec<String> = snapshot.viewport.iter().map(|row| row_to_string(row)).collect();
    let last_non_empty = lines.iter().rposition(|line| !line.is_empty());
    match last_non_empty {
        Some(index) => lines.truncate(index + 1),
        None => lines.clear(),
    }
    lines
}

/// Every cell's character, row by row.
///
/// # Errors
/// Returns [`Error::NoOutputAvailable`] when the viewport has no rows or no
/// columns.
pub fn snapshot_to_chars(id: Uuid, snapshot: &ix_vt::Snapshot) -> Result<Vec<Vec<char>>> {
    if snapshot.rows == 0 || snapshot.cols == 0 {
        return Err(Error::NoOutputAvailable { id });
    }

    let chars = snapshot
        .viewport
        .iter()
        .map(|row| row.iter().map(|cell| cell.ch.unwrap_or(' ')).collect())
        .collect();
    Ok(chars)
}

/// One styled cell from an `ix_vt` cell.
fn styled_cell(cell: &ix_vt::Cell) -> StyledCell {
    StyledCell {
        character: cell.ch.unwrap_or(' '),
        fg: crate::types::Color::from(cell.style.fg_color),
        bg: crate::types::Color::from(cell.style.bg_color),
        bold: cell.style.bold,
        italic: cell.style.italic,
        underline: cell.style.underline.is_some(),
        inverse: cell.style.inverse,
    }
}

/// The viewport as a `rows x cols` styled-cell array.
///
/// # Errors
/// Returns [`Error::NoOutputAvailable`] when the viewport has no rows or no
/// columns, or [`Error::ArrayConversion`] if the flattened cells do not fit the
/// declared shape.
pub fn snapshot_to_styled_cells(
    id: Uuid,
    snapshot: &ix_vt::Snapshot,
) -> Result<ndarray::Array2<StyledCell>> {
    let rows = usize::from(snapshot.rows);
    let cols = usize::from(snapshot.cols);
    if rows == 0 || cols == 0 {
        return Err(Error::NoOutputAvailable { id });
    }

    let mut data = Vec::with_capacity(rows * cols);
    for row in &snapshot.viewport {
        for cell in row {
            data.push(styled_cell(cell));
        }
    }

    ndarray::Array2::from_shape_vec((rows, cols), data).map_err(|source| Error::ArrayConversion {
        rows,
        cols,
        source,
    })
}

/// The cursor's `(row, col, visible)` in viewport cell coordinates.
///
/// The snapshot stores the cursor position as `(col, row)`, so it is swapped
/// here. A cursor scrolled out of the viewport reports `(0, 0)`.
pub fn snapshot_to_cursor(snapshot: &ix_vt::Snapshot) -> (u16, u16, bool) {
    let cursor = snapshot.cursor;
    let (row, col) = cursor.viewport.map_or((0, 0), |(x, y)| (y, x));
    (row, col, cursor.visible)
}
