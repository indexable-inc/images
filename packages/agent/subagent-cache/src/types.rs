//! Wire types shared by the `/lookup` and `/populate` endpoints.
//!
//! The daemon never touches the filesystem: clients compute every file hash
//! (they alone can see the working tree), so `file_deps` hashes are opaque
//! strings here. Stage-3 freshness (re-hashing those deps against disk) and the
//! persona check are owned by the client hook; the daemon owns Stage-1 recall
//! and Stage-2 judging only.

use serde::{Deserialize, Serialize};

/// One file the subagent read, with the content hash recorded at populate time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDep {
    pub path: String,
    /// mgrep's `xxh64:<16 hex>` format, treated opaquely by the daemon.
    pub hash: String,
}

/// `POST /lookup` request body.
#[derive(Debug, Clone, Deserialize)]
pub struct LookupRequest {
    pub agent_type: String,
    pub prompt: String,
    /// xxh64 of the client's local `.claude/agents/<agent_type>.md`. Folds the
    /// persona check into the recall filter.
    pub agent_def_hash: String,
}

/// One judge-positive candidate returned to the client for local freshness
/// validation.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    pub id: uuid::Uuid,
    pub findings: String,
    pub file_deps: Vec<FileDep>,
}

/// `POST /lookup` response: judge-positive candidates, best-first. An empty list
/// means the client should run the subagent cold.
#[derive(Debug, Clone, Serialize)]
pub struct LookupResponse {
    pub candidates: Vec<Candidate>,
}

/// `POST /populate` request body: one finished investigation.
#[derive(Debug, Clone, Deserialize)]
pub struct PopulateRequest {
    pub agent_type: String,
    pub prompt: String,
    pub findings: String,
    pub agent_def_hash: String,
    pub model: Option<String>,
    pub file_deps: Vec<FileDep>,
}

/// `POST /outcome` request body: the client-side Stage-3 resolution for one
/// lookup attempt. This is log-only telemetry; the daemon does not persist it.
#[derive(Debug, Clone, Deserialize)]
pub struct OutcomeRequest {
    pub agent_type: String,
    pub outcome: Outcome,
    pub candidate_id: Option<uuid::Uuid>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Served,
    Stale,
    Oversize,
    Miss,
}

impl Outcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Served => "served",
            Self::Stale => "stale",
            Self::Oversize => "oversize",
            Self::Miss => "miss",
        }
    }
}
