use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{Error, actor::PtyCommand, error::Result};

pub struct FullOutput {
    pub scrollback: Vec<String>,
    pub viewport: Vec<String>,
}

const POLL_INTERVAL_MS: u64 = 50;

pub async fn read_viewport_async(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Vec<String>> {
    let (response_tx, response_rx) = oneshot::channel();

    command_tx
        .send(PtyCommand::ReadViewport {
            response: response_tx,
        })
        .await
        .map_err(|_| Error::TuiNotFound { id })?;

    response_rx.await.map_err(|_| Error::TuiNotFound { id })?
}

pub async fn read_scrollback_async(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Vec<String>> {
    let (response_tx, response_rx) = oneshot::channel();

    command_tx
        .send(PtyCommand::ReadScrollback {
            response: response_tx,
        })
        .await
        .map_err(|_| Error::TuiNotFound { id })?;

    response_rx.await.map_err(|_| Error::TuiNotFound { id })?
}

pub async fn read_full_async(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<FullOutput> {
    let scrollback = read_scrollback_async(id, command_tx).await?;
    let viewport = read_viewport_async(id, command_tx).await?;
    Ok(FullOutput {
        scrollback,
        viewport,
    })
}

pub async fn read_blocking_async(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
    timeout_ms: u64,
) -> Result<Vec<String>> {
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(timeout_ms);
    let poll = tokio::time::Duration::from_millis(POLL_INTERVAL_MS);

    loop {
        match read_viewport_async(id, command_tx).await {
            Ok(lines) => return Ok(lines),
            Err(Error::NoOutputAvailable { .. }) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(poll).await;
            }
            Err(err) => return Err(err),
        }
    }
}

pub fn read_output(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Vec<String>> {
    read_viewport(runtime, id, command_tx)
}

pub fn read_viewport(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Vec<String>> {
    runtime.block_on(read_viewport_async(id, command_tx))
}

pub fn read_scrollback(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Vec<String>> {
    runtime.block_on(read_scrollback_async(id, command_tx))
}

pub fn read_full(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<FullOutput> {
    runtime.block_on(read_full_async(id, command_tx))
}

pub fn read_output_blocking(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
    timeout_ms: u64,
) -> Result<Vec<String>> {
    runtime.block_on(read_blocking_async(id, command_tx, timeout_ms))
}
