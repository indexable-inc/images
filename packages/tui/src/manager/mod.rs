pub mod reader;
mod spawn;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::runtime::Runtime;
use uuid::Uuid;

use crate::{Error, error::Result, types::TuiInstance};

pub struct TuiManager {
    instances: Arc<RwLock<HashMap<Uuid, TuiInstance>>>,
    runtime: Arc<Runtime>,
}

impl Default for TuiManager {
    fn default() -> Self {
        Self::new()
    }
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
        let id = instance.id;
        let command_tx = instance.command_tx.clone();
        let data = data.as_bytes().to_vec();
        let runtime = Arc::clone(&self.runtime);

        runtime.block_on(async move {
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            command_tx
                .send(crate::actor::PtyCommand::Write {
                    data,
                    response: response_tx,
                })
                .await
                .map_err(|_| Error::TuiNotFound { id })?;

            response_rx.await.map_err(|_| Error::TuiNotFound { id })??;

            Ok(())
        })
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
}
