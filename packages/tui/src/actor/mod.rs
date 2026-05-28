mod extract;

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{RwLock, mpsc, oneshot};
use uuid::Uuid;

use crate::{Error, error::Result};
use extract::{
    extract_chars, extract_scrollback_lines, extract_styled_cells, extract_viewport_lines,
};

pub enum PtyCommand {
    Write {
        data: Vec<u8>,
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
}

pub async fn pty_actor(
    id: Uuid,
    mut pty: pty_process::Pty,
    mut commands: mpsc::Receiver<PtyCommand>,
    parser: Arc<RwLock<vt100::Parser>>,
) {
    let mut read_buffer = [0u8; 8192];
    let mut pty_active = true;

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
                    PtyCommand::ReadViewport { response } => {
                        let parser_guard = parser.read().await;
                        let result = extract_viewport_lines(id, &parser_guard);
                        let _ = response.send(result);
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
                }
            }

            result = pty.read(&mut read_buffer), if pty_active => {
                match result {
                    Ok(0) | Err(_) => {
                        pty_active = false;
                    }
                    Ok(n) => {
                        let mut parser_guard = parser.write().await;
                        #[allow(clippy::indexing_slicing, reason = "n is guaranteed to be <= read_buffer.len() by read()")]
                        parser_guard.process(&read_buffer[..n]);
                    }
                }
            }

            else => break,
        }
    }
}
