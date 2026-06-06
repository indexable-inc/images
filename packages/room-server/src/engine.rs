//! The model-agnostic engine contract.
//!
//! This is the narrow interface behind which a wildly different transport
//! hides: Codex speaks a rich JSON-RPC app-server protocol; Claude speaks
//! streaming JSON over a one-shot subprocess. Both implement [`Engine`]
//! and emit the same [`EngineEvent`] union, so the bridge, HTTP layer, and
//! shared state never name a concrete engine.
//!
//! The event union is the superset of what Codex emits; Claude is a subset
//! producer. With `--dangerously-skip-permissions` Claude self-executes its
//! own tools and therefore never emits [`EngineEventBody::ApprovalRequest`]
//! or [`EngineEventBody::ToolCallRequest`]. Consumers stay identical: the
//! thinner engine accommodates the interface by omission, not by a
//! different shape.
//!
//! The serialized form here is the cross-language contract with the Elixir
//! runtime (`SymphonyElixir.Engine.*` and `SymphonyElixir.IR.*`). Field
//! casing is camelCase to match the existing room-server wire; enum bodies
//! carry a `type` tag. Golden fixtures under `tests/` keep both sides from
//! drifting.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Which engine kind an adapter is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineKind {
    Codex,
    Claude,
}

/// Engine-agnostic permission level. Each adapter lowers this to its native
/// shape: Codex to a sandbox policy plus approval policy, Claude to a
/// permission mode or `--dangerously-skip-permissions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permissions {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

/// Reasoning budget. `None` means let the engine pick its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

/// A request to begin one turn. The engine-agnostic form the Elixir
/// `Engine.Client` submits; each adapter lowers it to engine-native flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRequest {
    pub engine: EngineKind,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    pub permissions: Permissions,
    pub cwd: String,
    pub prompt: String,
    /// Dynamic-tool specs the host will execute on the engine's behalf.
    /// Empty for engines that self-execute their tools.
    #[serde(default)]
    pub tools: Vec<serde_json::Value>,
    /// Correlation ids from the runtime, echoed back on every event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

/// Handle identifying a live turn (the engine's thread/session id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnHandle {
    pub thread_id: String,
}

/// Status of a turn. The restart-reattach probe: a node found `running`
/// after a BEAM restart asks the engine for the status of its thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Idle,
    Running,
    AwaitingInput,
    Errored,
    Cancelled,
}

/// Terminal outcome of a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnOutcome {
    Ok,
    Error { message: String },
    Cancelled,
}

/// Id of a server-initiated request (approval or tool call) the host must
/// answer. Mirrors codex's i64 request ids.
pub type RequestId = i64;

/// What kind of approval the engine is asking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    CommandExecution,
    FileChange,
}

/// A file change reported by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
}

/// Token/cost accounting for a turn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default)]
    pub tokens_in: u64,
    #[serde(default)]
    pub tokens_out: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_creation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

/// The host's answer to a server-initiated request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineAnswer {
    /// Approve (optionally for the rest of the session).
    Approve {
        #[serde(default)]
        for_session: bool,
    },
    /// Reject the request.
    Deny,
    /// Result of a tool the host executed on the engine's behalf.
    ToolResult { result: serde_json::Value },
}

/// One normalized event from any engine, for one turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineEvent {
    pub turn_id: String,
    /// Monotonic per turn, for ordering and dedup on reattach.
    pub seq: u64,
    pub body: EngineEventBody,
}

/// The canonical event union. Codex maps its rich `item/*` notifications
/// onto this; Claude maps its stream-json events onto a subset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum EngineEventBody {
    TurnStarted {
        thread_id: String,
    },
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCallStarted {
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolCallOutput {
        call_id: String,
        output: serde_json::Value,
        #[serde(default)]
        delta: bool,
    },
    FileChanged {
        path: String,
        change: ChangeKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
    },
    StatusChanged {
        status: TurnStatus,
    },
    Usage {
        usage: Usage,
    },
    ApprovalRequest {
        req_id: RequestId,
        kind: ApprovalKind,
        detail: serde_json::Value,
    },
    ToolCallRequest {
        req_id: RequestId,
        name: String,
        args: serde_json::Value,
    },
    TurnCompleted {
        outcome: TurnOutcome,
    },
}

/// The deep module. Each adapter hides its transport behind these methods;
/// the bridge, state, and HTTP layers depend only on this trait and the
/// event union above.
pub trait Engine: Send + Sync {
    /// Which engine kind this adapter is.
    fn kind(&self) -> EngineKind;

    /// Begin a turn. Returns a handle identifying the live turn.
    fn start_turn(
        &self,
        turn: TurnRequest,
    ) -> impl std::future::Future<Output = anyhow::Result<TurnHandle>> + Send;

    /// Subscribe to the normalized event stream for all turns this engine
    /// instance owns. Subscribers filter by `turn_id`.
    fn subscribe(&self) -> broadcast::Receiver<EngineEvent>;

    /// Answer a server-initiated approval or tool-call request.
    fn answer(
        &self,
        req_id: RequestId,
        answer: EngineAnswer,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// Interrupt the active turn on a thread.
    fn interrupt(
        &self,
        turn: &TurnHandle,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// Current status of a turn (poll-based clients and restart reattach).
    fn status(
        &self,
        turn: &TurnHandle,
    ) -> impl std::future::Future<Output = anyhow::Result<TurnStatus>> + Send;

    /// Resolves when the underlying engine process exits, for supervisor
    /// respawn.
    fn wait_for_exit(&self) -> impl std::future::Future<Output = ()> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_event_round_trips_through_json() {
        let event = EngineEvent {
            turn_id: "thread_abc".to_owned(),
            seq: 7,
            body: EngineEventBody::TextDelta {
                text: "hello".to_owned(),
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"textDelta\""));
        assert!(json.contains("\"turnId\":\"thread_abc\""));

        let back: EngineEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn turn_request_round_trips_and_omits_none_effort() {
        let req = TurnRequest {
            engine: EngineKind::Claude,
            model: "haiku".to_owned(),
            effort: None,
            permissions: Permissions::DangerFullAccess,
            cwd: "/workspace".to_owned(),
            prompt: "write FOO".to_owned(),
            tools: vec![],
            run_id: Some("run_1".to_owned()),
            node_id: Some("n0".to_owned()),
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("effort"));
        assert!(json.contains("\"engine\":\"claude\""));
        assert!(json.contains("\"permissions\":\"danger_full_access\""));

        let back: TurnRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, "haiku");
        assert_eq!(back.engine, EngineKind::Claude);
    }

    #[test]
    fn completed_outcome_tags_kind() {
        let body = EngineEventBody::TurnCompleted {
            outcome: TurnOutcome::Ok,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"type\":\"turnCompleted\""));
        assert!(json.contains("\"kind\":\"ok\""));
    }
}
