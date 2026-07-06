//! On-disk journal: effect cache plus run history, one JSON file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use efx_ir::{Edge, EffectId};
use serde::{Deserialize, Serialize};
use snafu::ResultExt;

use crate::{EngineError, Outputs};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Succeeded,
    Failed,
}

/// The cached record of one effect identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEntry {
    pub name: String,
    pub kind: String,
    pub outputs: Outputs,
    pub status: Status,
    /// Unix seconds.
    pub recorded_at: u64,
}

/// What happened to one effect during one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Executed,
    Cached,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunEffect {
    pub name: String,
    pub kind: String,
    pub id: EffectId,
    pub action: Action,
    /// Why the effect executed (or failed); absent for plain cache hits.
    pub reason: Option<String>,
    pub duration_ms: u128,
    /// Per-input signatures, kept so the next run can explain what changed.
    pub input_signatures: BTreeMap<String, String>,
}

/// One recorded `apply`, in plan topological order.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunRecord {
    /// Unix seconds.
    pub recorded_at: u64,
    pub effects: Vec<RunEffect>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct JournalState {
    /// Effect cache, keyed by hex `EffectId`.
    pub entries: BTreeMap<String, JournalEntry>,
    pub runs: Vec<RunRecord>,
}

/// The journal file: loaded eagerly, saved atomically.
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    pub state: JournalState,
}

impl Journal {
    /// Loads the journal at `path`; a missing file is an empty journal.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::JournalRead`] on IO failure and
    /// [`EngineError::JournalFormat`] when the file is not journal JSON.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, EngineError> {
        let path = path.into();
        let display = path.display().to_string();
        let state = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .context(crate::JournalFormatSnafu { path: display })?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => JournalState::default(),
            Err(err) => {
                return Err(err).context(crate::JournalReadSnafu { path: display });
            }
        };
        Ok(Self { path, state })
    }

    /// Writes the journal back to its path via a sibling temp file + rename,
    /// so a crashed save never truncates history.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::JournalWrite`] on IO failure.
    pub fn save(&self) -> Result<(), EngineError> {
        let display = self.path.display().to_string();
        let json = serde_json::to_vec_pretty(&self.state)
            .unwrap_or_else(|_| unreachable!("journal state serializes"));
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json).context(crate::JournalWriteSnafu {
            path: display.clone(),
        })?;
        std::fs::rename(&tmp, &self.path).context(crate::JournalWriteSnafu { path: display })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn entry(&self, id: &EffectId) -> Option<&JournalEntry> {
        self.state.entries.get(&id.to_hex())
    }

    /// Whether `id` has a successful record — the memoization test.
    #[must_use]
    pub fn is_cached(&self, id: &EffectId) -> bool {
        self.entry(id)
            .is_some_and(|entry| entry.status == Status::Succeeded)
    }
}

/// Current wall-clock time as unix seconds.
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
