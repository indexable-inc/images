pub mod engine;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use crate::types::ExitState;
use crate::{Error, error::Result};
use engine::{
    EngineRequest, snapshot_to_chars, snapshot_to_cursor, snapshot_to_styled_cells,
    snapshot_to_viewport_lines,
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
/// The VT engine lives on its own OS thread (the terminal is `!Send`); the actor
/// forwards bytes and read requests to it through `engine_tx` and never touches
/// the terminal directly.
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
    engine_tx: std::sync::mpsc::Sender<EngineRequest>,
    exit_tx: watch::Sender<ExitState>,
) {
    let mut read_buffer = [0u8; 8192];
    let mut pty_active = true;
    let mut child_exited = false;

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
                            // engine so reads reflect the new geometry.
                            let result = pty
                                .resize(pty_process::Size::new(rows, cols))
                                .map_err(|e| Error::ResizeTui {
                                    id,
                                    source: std::io::Error::other(e),
                                });
                            let result = match result {
                                Ok(()) => resize_engine(id, &engine_tx, rows, cols).await,
                                Err(e) => Err(e),
                            };
                            let _ = response.send(result);
                        } else {
                            let _ = response.send(Err(Error::TuiNotFound { id }));
                        }
                    }
                    PtyCommand::ReadViewport { response } => {
                        let result = snapshot(id, &engine_tx)
                            .await
                            .map(|snap| snapshot_to_viewport_lines(&snap));
                        let _ = response.send(result);
                    }
                    PtyCommand::ReadScrollback { response } => {
                        let result = scrollback(id, &engine_tx).await;
                        let _ = response.send(result);
                    }
                    PtyCommand::ReadChars { response } => {
                        let result = snapshot(id, &engine_tx)
                            .await
                            .and_then(|snap| snapshot_to_chars(id, &snap));
                        let _ = response.send(result);
                    }
                    PtyCommand::ReadStyledCells { response } => {
                        let result = snapshot(id, &engine_tx)
                            .await
                            .and_then(|snap| snapshot_to_styled_cells(id, &snap));
                        let _ = response.send(result);
                    }
                    PtyCommand::ReadCursor { response } => {
                        let result = snapshot(id, &engine_tx)
                            .await
                            .map(|snap| snapshot_to_cursor(&snap));
                        let _ = response.send(result);
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
                        // Forward to the engine thread, which owns the !Send
                        // terminal. A closed channel means the engine is gone,
                        // so stop feeding the PTY.
                        if engine_tx.send(EngineRequest::Process(chunk.to_vec())).is_err() {
                            pty_active = false;
                        }
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

/// Round-trip a render snapshot through the engine thread.
///
/// A closed channel or a dropped reply means the engine thread is gone, which
/// surfaces as [`Error::TuiNotFound`]; a render failure surfaces as the inner
/// [`Error::VtEngine`].
async fn snapshot(
    id: Uuid,
    engine_tx: &std::sync::mpsc::Sender<EngineRequest>,
) -> Result<ix_vt::Snapshot> {
    let (reply, response) = oneshot::channel();
    engine_tx
        .send(EngineRequest::Snapshot { reply })
        .map_err(|_| Error::TuiNotFound { id })?;
    response.await.map_err(|_| Error::TuiNotFound { id })?
}

/// Read the scrollback history through the engine thread.
async fn scrollback(
    id: Uuid,
    engine_tx: &std::sync::mpsc::Sender<EngineRequest>,
) -> Result<Vec<String>> {
    let (reply, response) = oneshot::channel();
    engine_tx
        .send(EngineRequest::Scrollback { reply })
        .map_err(|_| Error::TuiNotFound { id })?;
    response.await.map_err(|_| Error::TuiNotFound { id })?
}

/// Resize the engine's terminal through the engine thread.
async fn resize_engine(
    id: Uuid,
    engine_tx: &std::sync::mpsc::Sender<EngineRequest>,
    rows: u16,
    cols: u16,
) -> Result<()> {
    let (reply, response) = oneshot::channel();
    engine_tx
        .send(EngineRequest::Resize { rows, cols, reply })
        .map_err(|_| Error::TuiNotFound { id })?;
    response.await.map_err(|_| Error::TuiNotFound { id })?
}
