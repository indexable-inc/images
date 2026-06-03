pub mod engine;

use std::sync::Arc;

use parking_lot::RwLock as SyncRwLock;
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
    app_cursor_keys: Arc<SyncRwLock<bool>>,
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
                            // A real terminal emits the cursor keys in
                            // application form once the program enables DECCKM;
                            // rewrite them here so callers send one spelling and
                            // full-screen programs still receive their arrows.
                            let data = apply_cursor_key_mode(&data, *app_cursor_keys.read());
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

/// Rewrite normal-mode cursor-key sequences into their application-mode form
/// when the program has enabled DECCKM.
///
/// A real terminal emits the cursor keys as `ESC O A`..`ESC O D` (and Home/End
/// as `ESC O H`/`ESC O F`) once the program enables DECCKM via terminfo `smkx`,
/// which ncurses, vim, and less all do on entry. Sending the normal `ESC [ A`
/// form instead leaves those programs blind to the arrows. These exact 3-byte
/// sequences only arise as terminal *input* from a cursor key, so the
/// substitution is unambiguous. A modified arrow carries parameters
/// (`ESC [ 1 ; 5 A` for Ctrl+Up), so the byte after `[` is a digit rather than
/// the final letter and the sequence is left untouched.
fn apply_cursor_key_mode(data: &[u8], application_mode: bool) -> Vec<u8> {
    if !application_mode {
        return data.to_vec();
    }

    let mut out = Vec::with_capacity(data.len());
    let mut rest = data;
    while let Some((&first, tail)) = rest.split_first() {
        if first == 0x1b
            && let [b'[', final_byte, remainder @ ..] = tail
            && matches!(*final_byte, b'A' | b'B' | b'C' | b'D' | b'H' | b'F')
        {
            out.extend_from_slice(&[0x1b, b'O', *final_byte]);
            rest = remainder;
        } else {
            out.push(first);
            rest = tail;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::apply_cursor_key_mode;

    #[test]
    fn normal_mode_passes_cursor_keys_through() {
        assert_eq!(apply_cursor_key_mode(b"\x1b[B", false), b"\x1b[B".to_vec());
    }

    #[test]
    fn application_mode_rewrites_cursor_and_home_end_keys() {
        assert_eq!(apply_cursor_key_mode(b"\x1b[A", true), b"\x1bOA".to_vec());
        assert_eq!(apply_cursor_key_mode(b"\x1b[B", true), b"\x1bOB".to_vec());
        assert_eq!(apply_cursor_key_mode(b"\x1b[C", true), b"\x1bOC".to_vec());
        assert_eq!(apply_cursor_key_mode(b"\x1b[D", true), b"\x1bOD".to_vec());
        assert_eq!(apply_cursor_key_mode(b"\x1b[H", true), b"\x1bOH".to_vec());
        assert_eq!(apply_cursor_key_mode(b"\x1b[F", true), b"\x1bOF".to_vec());
    }

    #[test]
    fn application_mode_leaves_text_and_modified_keys_untouched() {
        assert_eq!(apply_cursor_key_mode(b"hello", true), b"hello".to_vec());
        // Modified arrow (Ctrl+Up = ESC [ 1 ; 5 A) keeps its CSI form.
        assert_eq!(
            apply_cursor_key_mode(b"\x1b[1;5A", true),
            b"\x1b[1;5A".to_vec()
        );
        // A cursor key surrounded by text is still rewritten in place.
        assert_eq!(
            apply_cursor_key_mode(b"a\x1b[Bb", true),
            b"a\x1bOBb".to_vec()
        );
    }

    #[test]
    fn application_mode_preserves_sequence_truncated_at_end() {
        // A buffer that ends mid-sequence has no final byte to match, so the
        // partial bytes are emitted verbatim rather than dropped or panicking.
        assert_eq!(apply_cursor_key_mode(b"\x1b", true), b"\x1b".to_vec());
        assert_eq!(apply_cursor_key_mode(b"\x1b[", true), b"\x1b[".to_vec());
        assert_eq!(apply_cursor_key_mode(b"x\x1b[", true), b"x\x1b[".to_vec());
    }

    #[test]
    fn rewrite_is_per_call_not_across_writes() {
        // Each write is rewritten independently: an arrow split across two
        // calls is not reassembled, so neither half is altered. Callers that
        // need a cursor key send it as one chunk.
        assert_eq!(apply_cursor_key_mode(b"\x1b[", true), b"\x1b[".to_vec());
        assert_eq!(apply_cursor_key_mode(b"B", true), b"B".to_vec());
    }
}
