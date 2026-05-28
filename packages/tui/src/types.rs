use std::time::SystemTime;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::actor::PtyCommand;

#[derive(Clone)]
pub struct TuiInstance {
    pub id: Uuid,
    pub command: String,
    pub args: Vec<String>,
    pub spawned_at: SystemTime,
    pub cols: u16,
    pub rows: u16,
    pub scrollback_limit: usize,
    pub(crate) command_tx: mpsc::Sender<PtyCommand>,
}

#[derive(Debug, Clone)]
pub struct StyledCell {
    pub character: char,
    pub fgcolor: Option<String>,
    pub bgcolor: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}
