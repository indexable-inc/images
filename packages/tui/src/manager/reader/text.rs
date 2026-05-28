use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{Error, actor::PtyCommand, error::Result};

pub struct FullOutput {
    pub scrollback: Vec<String>,
    pub viewport: Vec<String>,
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
    runtime.block_on(async {
        let (response_tx, response_rx) = oneshot::channel();

        command_tx
            .send(PtyCommand::ReadViewport {
                response: response_tx,
            })
            .await
            .map_err(|_| Error::TuiNotFound { id })?;

        response_rx.await.map_err(|_| Error::TuiNotFound { id })?
    })
}

pub fn read_scrollback(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Vec<String>> {
    runtime.block_on(async {
        let (response_tx, response_rx) = oneshot::channel();

        command_tx
            .send(PtyCommand::ReadScrollback {
                response: response_tx,
            })
            .await
            .map_err(|_| Error::TuiNotFound { id })?;

        response_rx.await.map_err(|_| Error::TuiNotFound { id })?
    })
}

pub fn read_full(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<FullOutput> {
    let scrollback = read_scrollback(runtime, id, command_tx)?;
    let viewport = read_viewport(runtime, id, command_tx)?;
    Ok(FullOutput {
        scrollback,
        viewport,
    })
}

const POLL_INTERVAL_MS: u64 = 50;

pub fn read_output_blocking(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
    timeout_ms: u64,
) -> Result<Vec<String>> {
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);

    loop {
        match read_output(runtime, id, command_tx) {
            Ok(lines) => return Ok(lines),
            Err(Error::NoOutputAvailable { .. }) if start.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));
            }
            Err(err) => return Err(err),
        }
    }
}
