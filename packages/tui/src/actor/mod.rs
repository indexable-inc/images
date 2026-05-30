mod decscusr;
mod extract;

use std::sync::Arc;
use parking_lot::RwLock as SyncRwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{RwLock, mpsc, oneshot, watch};
use uuid::Uuid;

use crate::types::{CursorShape, ExitState};
use crate::{Error, error::Result};
use decscusr::Scanner;
use extract::{
    extract_chars, extract_cursor, extract_scrollback_lines, extract_styled_cells,
    extract_viewport_lines,
};

pub enum PtyCommand {
    Write {
        data: Vec<u8>,
        response: oneshot::Sender<Result<()>>,
    },
    Kill {
        response: oneshot::Sender<Result<()>>,
    },
    Resize {
        rows: u16,
        cols: u16,
        response: oneshot::Sender<Result<()>>,
    },
    ReadViewport {
        response: oneshot::Sender<Result<Vec<String>>>,
    },
    ReadScrollback {
        response: oneshot::Sender<Result<Vec<String>>>,
    },
    ReadChars {
        response: oneshot::Sender<Result<Vec<Vec<char>>>>,
    },
    ReadStyledCells {
        response: oneshot::Sender<Result<ndarray::Array2<crate::types::StyledCell>>>,
    },
    ReadCursor {
        response: oneshot::Sender<Result<(u16, u16, bool)>>,
    },
}

/// Owns the PTY master and the child process for one terminal. It is the only
/// task that touches either, so every read, write, signal, and the exit reap
/// serialize through this one mailbox.
///
/// The child is reaped here rather than in a side task so its exit code lands
/// in `exit_tx` and a [`PtyCommand::Kill`] has something to signal. After the
/// child exits the actor keeps serving reads (the final screen stays
/// inspectable) and stays alive until every handle is dropped.
pub async fn pty_actor(
    id: Uuid,
    mut pty: pty_process::Pty,
    mut child: tokio::process::Child,
    mut commands: mpsc::Receiver<PtyCommand>,
    parser: Arc<RwLock<vt100::Parser>>,
    exit_tx: watch::Sender<ExitState>,
    cursor_shape: Arc<SyncRwLock<CursorShape>>,
) {
    let mut read_buffer = [0u8; 8192];
    let mut pty_active = true;
    let mut child_exited = false;
    let mut scanner = Scanner::default();

    loop {
        tokio::select! {
            biased;

            Some(cmd) = commands.recv() => {
                match cmd {
                    PtyCommand::Write { data, response } => {
                        if pty_active {
                            let result = pty.write_all(&data)
                                .await
                                .map_err(|e| crate::Error::WriteToTui {
                                    id,
                                    source: e,
                                });
                            let _ = response.send(result);
                        } else {
                            let _ = response.send(Err(Error::TuiNotFound { id }));
                        }
                    }
                    PtyCommand::Kill { response } => {
                        // start_kill on an already-reaped child is a harmless
                        // no-op, so a redundant kill still reports success.
                        let result = child.start_kill()
                            .map_err(|e| Error::SignalTui { id, source: e });
                        let _ = response.send(result);
                    }
                    PtyCommand::Resize { rows, cols, response } => {
                        if pty_active {
                            // Resize the kernel PTY window first (this is what
                            // delivers SIGWINCH to the child), then match the
                            // emulator so reads reflect the new geometry.
                            let result = pty
                                .resize(pty_process::Size::new(rows, cols))
                                .map_err(|e| Error::ResizeTui {
                                    id,
                                    source: std::io::Error::other(e),
                                });
                            if result.is_ok() {
                                parser.write().await.screen_mut().set_size(rows, cols);
                            }
                            let _ = response.send(result);
                        } else {
                            let _ = response.send(Err(Error::TuiNotFound { id }));
                        }
                    }
                    PtyCommand::ReadViewport { response } => {
                        let parser_guard = parser.read().await;
                        let lines = extract_viewport_lines(&parser_guard);
                        let _ = response.send(Ok(lines));
                    }
                    PtyCommand::ReadScrollback { response } => {
                        let mut parser_guard = parser.write().await;
                        let result = extract_scrollback_lines(&mut parser_guard);
                        let _ = response.send(Ok(result));
                    }
                    PtyCommand::ReadChars { response } => {
                        let parser_guard = parser.read().await;
                        let result = extract_chars(id, &parser_guard);
                        let _ = response.send(result);
                    }
                    PtyCommand::ReadStyledCells { response } => {
                        let parser_guard = parser.read().await;
                        let result = extract_styled_cells(id, &parser_guard);
                        let _ = response.send(result);
                    }
                    PtyCommand::ReadCursor { response } => {
                        let parser_guard = parser.read().await;
                        let cursor = extract_cursor(&parser_guard);
                        let _ = response.send(Ok(cursor));
                    }
                }
            }

            result = pty.read(&mut read_buffer), if pty_active => {
                match result {
                    Ok(0) | Err(_) => {
                        pty_active = false;
                    }
                    Ok(n) => {
                        #[allow(clippy::indexing_slicing, reason = "n is guaranteed to be <= read_buffer.len() by read()")]
                        let chunk = &read_buffer[..n];
                        // vt100 applies cursor position but not cursor shape, so
                        // sniff DECSCUSR from the same bytes before feeding them.
                        if let Some(shape) = scanner.feed(chunk) {
                            *cursor_shape.write() = shape;
                        }
                        parser.write().await.process(chunk);
                    }
                }
            }

            // `Child::wait` is cancel-safe, so racing it in the select is fine;
            // once it resolves we disable the branch and publish the exit code.
            status = child.wait(), if !child_exited => {
                child_exited = true;
                let code = status.ok().and_then(|status| status.code());
                let _ = exit_tx.send(ExitState::Exited(code));
            }

            else => break,
        }
    }
}
