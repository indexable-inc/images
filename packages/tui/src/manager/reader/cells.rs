use ndarray::Array2;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{Error, actor::PtyCommand, error::Result, types::StyledCell};

pub async fn read_chars_async(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Vec<Vec<char>>> {
    let (response_tx, response_rx) = oneshot::channel();

    command_tx
        .send(PtyCommand::ReadChars {
            response: response_tx,
        })
        .await
        .map_err(|_| Error::TuiNotFound { id })?;

    response_rx.await.map_err(|_| Error::TuiNotFound { id })?
}

pub async fn read_styled_cells_async(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Array2<StyledCell>> {
    let (response_tx, response_rx) = oneshot::channel();

    command_tx
        .send(PtyCommand::ReadStyledCells {
            response: response_tx,
        })
        .await
        .map_err(|_| Error::TuiNotFound { id })?;

    response_rx.await.map_err(|_| Error::TuiNotFound { id })?
}

pub fn read_chars(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Vec<Vec<char>>> {
    runtime.block_on(read_chars_async(id, command_tx))
}

pub fn read_styled_cells(
    runtime: &Arc<Runtime>,
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
) -> Result<Array2<StyledCell>> {
    runtime.block_on(read_styled_cells_async(id, command_tx))
}
