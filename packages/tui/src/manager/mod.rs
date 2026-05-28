pub mod reader;
mod spawn;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::{Error, actor::PtyCommand, error::Result, types::TuiInstance};

pub struct TuiManager {
    instances: Arc<RwLock<HashMap<Uuid, TuiInstance>>>,
    runtime: Arc<Runtime>,
}

impl Default for TuiManager {
    fn default() -> Self {
        Self::new()
    }
}

async fn write_async_impl(
    id: Uuid,
    command_tx: &mpsc::Sender<PtyCommand>,
    data: Vec<u8>,
) -> Result<()> {
    let (response_tx, response_rx) = oneshot::channel();

    command_tx
        .send(PtyCommand::Write {
            data,
            response: response_tx,
        })
        .await
        .map_err(|_| Error::TuiNotFound { id })?;

    response_rx.await.map_err(|_| Error::TuiNotFound { id })??;

    Ok(())
}

impl TuiManager {
    #[must_use]
    /// # Panics
    /// Panics if the Tokio runtime cannot be created.
    pub fn new() -> Self {
        let runtime = Runtime::new().unwrap_or_else(|_| {
            panic!("failed to create tokio runtime for TUI manager");
        });

        Self {
            instances: Arc::new(RwLock::new(HashMap::new())),
            runtime: Arc::new(runtime),
        }
    }

    pub fn spawn(
        &self,
        command: String,
        args: Vec<String>,
        scrollback_lines: usize,
    ) -> Result<TuiInstance> {
        let instance = spawn::spawn_tui(&self.runtime, command, args, scrollback_lines)?;
        let id = instance.id;
        self.instances.write().insert(id, instance.clone());
        Ok(instance)
    }

    #[must_use]
    pub fn list(&self) -> Vec<TuiInstance> {
        self.instances.read().values().cloned().collect()
    }

    pub fn get(&self, id: &Uuid) -> Result<TuiInstance> {
        self.instances
            .read()
            .get(id)
            .cloned()
            .ok_or(Error::TuiNotFound { id: *id })
    }

    pub fn write(&self, instance: &TuiInstance, data: &str) -> Result<()> {
        self.runtime.block_on(write_async_impl(
            instance.id,
            &instance.command_tx,
            data.as_bytes().to_vec(),
        ))
    }

    pub fn read(&self, instance: &TuiInstance) -> Result<Vec<String>> {
        reader::read_output(&self.runtime, instance.id, &instance.command_tx)
    }

    pub fn read_viewport(&self, instance: &TuiInstance) -> Result<Vec<String>> {
        reader::read_viewport(&self.runtime, instance.id, &instance.command_tx)
    }

    pub fn read_scrollback(&self, instance: &TuiInstance) -> Result<Vec<String>> {
        reader::read_scrollback(&self.runtime, instance.id, &instance.command_tx)
    }

    pub fn read_blocking(&self, instance: &TuiInstance, timeout_ms: u64) -> Result<Vec<String>> {
        reader::read_output_blocking(&self.runtime, instance.id, &instance.command_tx, timeout_ms)
    }

    pub fn read_full(&self, instance: &TuiInstance) -> Result<reader::FullOutput> {
        reader::read_full(&self.runtime, instance.id, &instance.command_tx)
    }

    pub fn read_chars(&self, instance: &TuiInstance) -> Result<Vec<Vec<char>>> {
        reader::read_chars(&self.runtime, instance.id, &instance.command_tx)
    }

    pub fn read_styled_cells(
        &self,
        instance: &TuiInstance,
    ) -> Result<ndarray::Array2<crate::types::StyledCell>> {
        reader::read_styled_cells(&self.runtime, instance.id, &instance.command_tx)
    }

    pub async fn write_async(instance: &TuiInstance, data: &str) -> Result<()> {
        write_async_impl(instance.id, &instance.command_tx, data.as_bytes().to_vec()).await
    }

    pub async fn read_viewport_async(instance: &TuiInstance) -> Result<Vec<String>> {
        reader::read_viewport_async(instance.id, &instance.command_tx).await
    }

    pub async fn read_scrollback_async(instance: &TuiInstance) -> Result<Vec<String>> {
        reader::read_scrollback_async(instance.id, &instance.command_tx).await
    }

    pub async fn read_full_async(instance: &TuiInstance) -> Result<reader::FullOutput> {
        reader::read_full_async(instance.id, &instance.command_tx).await
    }

    pub async fn read_blocking_async(
        instance: &TuiInstance,
        timeout_ms: u64,
    ) -> Result<Vec<String>> {
        reader::read_blocking_async(instance.id, &instance.command_tx, timeout_ms).await
    }

    pub async fn read_chars_async(instance: &TuiInstance) -> Result<Vec<Vec<char>>> {
        reader::read_chars_async(instance.id, &instance.command_tx).await
    }

    pub async fn read_styled_cells_async(
        instance: &TuiInstance,
    ) -> Result<ndarray::Array2<crate::types::StyledCell>> {
        reader::read_styled_cells_async(instance.id, &instance.command_tx).await
    }
}
