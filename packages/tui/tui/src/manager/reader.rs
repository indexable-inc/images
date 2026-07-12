//! Low-level request helpers: issue one [`PtyCommand`] and await its reply.
//!
//! Every read and write is the same handshake: build a command carrying a
//! fresh oneshot sender, push it onto the actor channel, and await the
//! response. [`request`] captures that handshake once; the public helpers just
//! name the command they send.

use std::time::Duration;

use ndarray::Array2;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::actor::PtyCommand;
use crate::types::{CursorPos, FullOutput, StyledCell};
use crate::{Error, Result};

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Send the command built by `build` and await its reply. A closed channel
/// (the actor has exited) surfaces as [`Error::TuiNotFound`].
async fn request<T>(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
    build: impl FnOnce(oneshot::Sender<Result<T>>) -> PtyCommand,
) -> Result<T> {
    let (response_tx, response_rx) = oneshot::channel();
    command_tx
        .send(build(response_tx))
        .await
        .map_err(|_| Error::TuiNotFound { id })?;
    response_rx.await.map_err(|_| Error::TuiNotFound { id })?
}

pub(super) async fn write(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
    data: Vec<u8>,
) -> Result<()> {
    request(id, command_tx, |response| PtyCommand::Write {
        data,
        response,
    })
    .await
}

pub(super) async fn kill(id: Uuid, command_tx: &mpsc::Sender<PtyCommand>) -> Result<()> {
    request(id, command_tx, |response| PtyCommand::Kill { response }).await
}

pub(super) async fn resize(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
    rows: u16,
    cols: u16,
) -> Result<()> {
    request(id, command_tx, |response| PtyCommand::Resize {
        rows,
        cols,
        response,
    })
    .await
}

macro_rules! read_request {
    ($name:ident, $variant:ident, $output:ty) => {
        pub(super) async fn $name(
            id: Uuid,
            command_tx: &mpsc::Sender<PtyCommand>,
        ) -> Result<$output> {
            request(id, command_tx, |response| PtyCommand::$variant { response }).await
        }
    };
}

read_request!(read_viewport, ReadViewport, Vec<String>);
read_request!(read_scrollback, ReadScrollback, Vec<String>);
read_request!(read_chars, ReadChars, Vec<Vec<char>>);
read_request!(read_styled_cells, ReadStyledCells, Array2<StyledCell>);
read_request!(read_cursor, ReadCursor, CursorPos);

pub(super) async fn read_full(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<FullOutput> {
    let scrollback = read_scrollback(id, command_tx).await?;
    let viewport = read_viewport(id, command_tx).await?;
    Ok(FullOutput {
        scrollback,
        viewport,
    })
}

pub(super) async fn read_blocking(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
    timeout: Duration,
) -> Result<Vec<String>> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let lines = read_viewport(id, command_tx).await?;
        if lines.iter().any(|line| !line.trim().is_empty()) {
            return Ok(lines);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::NoOutputAvailable { id });
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
