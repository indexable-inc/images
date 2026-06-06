//! Codex adapter for the [`Engine`] trait.
//!
//! This wraps the existing [`CodexClient`] (the JSON-RPC client for
//! `codex app-server`) and translates codex's rich `item/*`, `turn/*`,
//! and `server/request` notifications into the canonical
//! [`EngineEvent`] union. The mapping is a mechanical lift of the
//! projection logic that `codex_bridge.rs` performs against the room
//! DB, narrowed here to the engine-agnostic event shape so that
//! `bridge.rs`, `state.rs`, and `http.rs` can consume codex turns
//! without naming codex.
//!
//! Lowering goes the other way: a [`TurnRequest`] becomes the
//! `thread/start` + `turn/start` params codex expects. `permissions`
//! lowers to a sandbox policy plus approval policy, `effort` to codex's
//! `ReasoningEffort` string, and `tools` to `dynamicTools`.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, broadcast};

use crate::{
    codex_rpc::{CodexClient, Notification},
    engine::{
        ChangeKind, Effort, Engine, EngineAnswer, EngineEvent, EngineEventBody, EngineKind,
        Permissions, RequestId, TurnHandle, TurnOutcome, TurnRequest, TurnStatus,
    },
};

/// One running `codex app-server` behind the [`Engine`] interface.
///
/// The translation task subscribes to the raw codex notification
/// stream once and re-broadcasts canonical [`EngineEvent`]s. Each
/// adapter owns one monotonic sequence counter; codex does not number
/// its notifications, so the adapter assigns `seq` as it observes them.
pub struct CodexEngine {
    client: Arc<CodexClient>,
    events: broadcast::Sender<EngineEvent>,
    /// Maps a server-initiated request id back to the codex method so
    /// `answer` can build the right reply shape. Codex distinguishes
    /// command-approval, patch-approval, and dynamic-tool requests by
    /// method, but the host answers them all through one [`EngineAnswer`].
    request_methods: Arc<Mutex<std::collections::HashMap<RequestId, String>>>,
}

impl CodexEngine {
    /// Wrap a live [`CodexClient`] and start the translation task. The
    /// task lives as long as the codex notification channel stays open.
    pub fn new(client: Arc<CodexClient>) -> Arc<Self> {
        let (events, _) = broadcast::channel::<EngineEvent>(1024);
        let seq = Arc::new(AtomicU64::new(0));
        let request_methods = Arc::new(Mutex::new(std::collections::HashMap::new()));

        let engine = Arc::new(Self {
            client: client.clone(),
            events: events.clone(),
            request_methods: request_methods.clone(),
        });

        let mut raw = client.subscribe();
        tokio::spawn(async move {
            loop {
                match raw.recv().await {
                    Ok(note) => {
                        for event in notification_to_events(&note, &seq) {
                            if let EngineEventBody::ApprovalRequest { req_id, .. }
                            | EngineEventBody::ToolCallRequest { req_id, .. } = &event.body
                            {
                                let method = note
                                    .params
                                    .get("method")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_owned();
                                request_methods.lock().await.insert(*req_id, method);
                            }
                            let _ = events.send(event);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        engine
    }
}

impl Engine for CodexEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Codex
    }

    async fn start_turn(&self, turn: TurnRequest) -> Result<TurnHandle> {
        let thread_id = start_thread(&self.client, &turn).await?;

        let params = turn_start_params(&thread_id, &turn);
        self.client
            .request("turn/start", Value::Object(params))
            .await
            .context("codex turn/start")?;

        Ok(TurnHandle { thread_id })
    }

    fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    async fn answer(&self, req_id: RequestId, answer: EngineAnswer) -> Result<()> {
        let method = self.request_methods.lock().await.remove(&req_id);
        let result = codex_reply(method.as_deref(), &answer);
        self.client.reply_server_request(req_id, result)
    }

    async fn interrupt(&self, turn: &TurnHandle) -> Result<()> {
        self.client.interrupt_active_turn(&turn.thread_id).await
    }

    async fn status(&self, turn: &TurnHandle) -> Result<TurnStatus> {
        // An active turn id tracked by the client means a turn is in
        // flight; absence means the thread is idle. This is the
        // restart-reattach probe: a node found `running` after a BEAM
        // restart asks here whether codex still owns the thread.
        Ok(match self.client.active_turn_id(&turn.thread_id) {
            Some(_) => TurnStatus::Running,
            None => TurnStatus::Idle,
        })
    }

    async fn wait_for_exit(&self) {
        self.client.wait_for_exit().await;
    }
}

// ---------------------------------------------------------------------
// Lowering: TurnRequest -> codex params
// ---------------------------------------------------------------------

async fn start_thread(client: &CodexClient, turn: &TurnRequest) -> Result<String> {
    let mut params = Map::new();
    if !turn.cwd.is_empty() {
        params.insert("cwd".to_owned(), Value::String(turn.cwd.clone()));
    }
    if !turn.model.is_empty() {
        params.insert("model".to_owned(), Value::String(turn.model.clone()));
    }
    params.insert(
        "sandbox".to_owned(),
        Value::String(sandbox_name(turn.permissions)),
    );
    if !turn.tools.is_empty() {
        params.insert("dynamicTools".to_owned(), Value::Array(turn.tools.clone()));
    }

    let res = client
        .request("thread/start", Value::Object(params))
        .await
        .context("codex thread/start")?;
    res.get("thread")
        .and_then(|t| t.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("thread/start response missing thread.id")
}

/// Build the `turn/start` params for a [`TurnRequest`] against an
/// existing codex thread. Exposed to tests so the permissions/effort
/// lowering is checked without a live codex binary.
pub fn turn_start_params(thread_id: &str, turn: &TurnRequest) -> Map<String, Value> {
    let mut params = Map::new();
    params.insert("threadId".to_owned(), Value::String(thread_id.to_owned()));
    params.insert(
        "input".to_owned(),
        json!([{ "type": "text", "text": turn.prompt }]),
    );
    if !turn.cwd.is_empty() {
        params.insert("cwd".to_owned(), Value::String(turn.cwd.clone()));
    }
    if !turn.model.is_empty() {
        params.insert("model".to_owned(), Value::String(turn.model.clone()));
    }
    params.insert("sandboxPolicy".to_owned(), sandbox_policy(turn.permissions));
    params.insert(
        "approvalPolicy".to_owned(),
        approval_policy(turn.permissions),
    );
    if let Some(effort) = turn.effort {
        params.insert("effort".to_owned(), Value::String(effort_name(effort)));
    }
    params
}

/// The `sandbox` shorthand codex's `thread/start` accepts.
fn sandbox_name(permissions: Permissions) -> String {
    match permissions {
        Permissions::ReadOnly => "read-only",
        Permissions::WorkspaceWrite => "workspace-write",
        Permissions::DangerFullAccess => "danger-full-access",
    }
    .to_owned()
}

/// The structured `sandboxPolicy` codex's `turn/start` accepts.
fn sandbox_policy(permissions: Permissions) -> Value {
    match permissions {
        Permissions::ReadOnly => json!({ "type": "readOnly", "networkAccess": true }),
        Permissions::WorkspaceWrite => json!({ "type": "workspaceWrite", "networkAccess": true }),
        Permissions::DangerFullAccess => json!({ "type": "dangerFullAccess" }),
    }
}

/// The approval policy paired with each sandbox. Full access runs
/// unattended (`never`); the writable and read-only sandboxes ask the
/// host on escalation so the host's [`EngineAnswer`] path stays live.
fn approval_policy(permissions: Permissions) -> Value {
    match permissions {
        Permissions::DangerFullAccess => json!("never"),
        Permissions::WorkspaceWrite | Permissions::ReadOnly => json!("on-request"),
    }
}

/// Mirrors codex's `ReasoningEffort` enum
/// (none/minimal/low/medium/high/xhigh).
fn effort_name(effort: Effort) -> String {
    match effort {
        Effort::None => "none",
        Effort::Minimal => "minimal",
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
    }
    .to_owned()
}

/// Build the JSON reply codex expects for a server-initiated request,
/// given the host's engine-agnostic [`EngineAnswer`]. Approval requests
/// take a `decision` string; a dynamic tool call takes a `result`.
fn codex_reply(method: Option<&str>, answer: &EngineAnswer) -> Value {
    match answer {
        EngineAnswer::Approve { for_session } => {
            let decision = if *for_session {
                "approvedForSession"
            } else {
                "approved"
            };
            json!({ "decision": decision })
        }
        EngineAnswer::Deny => json!({ "decision": "denied" }),
        EngineAnswer::ToolResult { result } => {
            let _ = method;
            json!({ "result": result })
        }
    }
}

// ---------------------------------------------------------------------
// Lifting: codex notification -> EngineEvent
// ---------------------------------------------------------------------

/// Translate one codex notification into zero or more canonical
/// [`EngineEvent`]s. Pure: it reads the notification JSON and the
/// shared sequence counter, and never touches the DB or the network.
/// This is the unit-tested seam.
pub fn notification_to_events(note: &Notification, seq: &AtomicU64) -> Vec<EngineEvent> {
    let params = &note.params;
    let bodies = notification_bodies(&note.method, params);
    if bodies.is_empty() {
        return Vec::new();
    }
    let turn_id = thread_id(params).unwrap_or("").to_owned();
    bodies
        .into_iter()
        .map(|body| EngineEvent {
            turn_id: turn_id.clone(),
            seq: seq.fetch_add(1, Ordering::Relaxed),
            body,
        })
        .collect()
}

fn notification_bodies(method: &str, params: &Value) -> Vec<EngineEventBody> {
    match method {
        "turn/started" => thread_id(params)
            .map(|id| {
                vec![EngineEventBody::TurnStarted {
                    thread_id: id.to_owned(),
                }]
            })
            .unwrap_or_default(),
        "item/agentMessage/delta" => text_delta(params)
            .map(|text| vec![EngineEventBody::TextDelta { text }])
            .unwrap_or_default(),
        "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => text_delta(params)
            .map(|text| vec![EngineEventBody::ReasoningDelta { text }])
            .unwrap_or_default(),
        "item/started" => item_started_bodies(params),
        "item/commandExecution/outputDelta" => command_output_delta(params)
            .map(|(call_id, output)| {
                vec![EngineEventBody::ToolCallOutput {
                    call_id,
                    output: Value::String(output),
                    delta: true,
                }]
            })
            .unwrap_or_default(),
        "item/completed" => item_completed_bodies(params),
        "thread/tokenUsage/updated" => token_usage(params)
            .map(|usage| vec![EngineEventBody::Usage { usage }])
            .unwrap_or_default(),
        "thread/status/changed" => status_changed(params)
            .map(|status| vec![EngineEventBody::StatusChanged { status }])
            .unwrap_or_default(),
        "turn/completed" => vec![EngineEventBody::TurnCompleted {
            outcome: TurnOutcome::Ok,
        }],
        "turn/failed" => {
            let message = params
                .get("error")
                .and_then(|e| e.get("message").or(Some(e)))
                .and_then(Value::as_str)
                .unwrap_or("turn failed")
                .to_owned();
            vec![EngineEventBody::TurnCompleted {
                outcome: TurnOutcome::Error { message },
            }]
        }
        "turn/cancelled" => vec![EngineEventBody::TurnCompleted {
            outcome: TurnOutcome::Cancelled,
        }],
        "server/request" => server_request_bodies(params),
        _ => Vec::new(),
    }
}

/// `item/started` becomes a `ToolCallStarted` for the tool-bearing item
/// kinds (commandExecution, fileChange, mcpToolCall, webSearch). Text
/// and reasoning items stream through their delta notifications, so they
/// produce no event here.
fn item_started_bodies(params: &Value) -> Vec<EngineEventBody> {
    let Some(item) = params.get("item") else {
        return Vec::new();
    };
    let Some(call_id) = item.get("id").and_then(Value::as_str) else {
        return Vec::new();
    };
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "commandExecution" => vec![EngineEventBody::ToolCallStarted {
            call_id: call_id.to_owned(),
            name: "shell".to_owned(),
            args: json!({ "command": item.get("command"), "cwd": item.get("cwd") }),
        }],
        "fileChange" => vec![EngineEventBody::ToolCallStarted {
            call_id: call_id.to_owned(),
            name: "apply_patch".to_owned(),
            args: item.get("changes").cloned().unwrap_or(Value::Null),
        }],
        "mcpToolCall" => vec![EngineEventBody::ToolCallStarted {
            call_id: call_id.to_owned(),
            name: format!(
                "{}::{}",
                item.get("server").and_then(Value::as_str).unwrap_or("mcp"),
                item.get("tool").and_then(Value::as_str).unwrap_or("?"),
            ),
            args: item.get("arguments").cloned().unwrap_or(Value::Null),
        }],
        "webSearch" => vec![EngineEventBody::ToolCallStarted {
            call_id: call_id.to_owned(),
            name: "web_search".to_owned(),
            args: item.get("query").cloned().unwrap_or(Value::Null),
        }],
        _ => Vec::new(),
    }
}

/// `item/completed` carries the authoritative item payload. Tool kinds
/// emit a final `ToolCallOutput`; a fileChange additionally emits a
/// `FileChanged` per changed path so consumers see the file events
/// without parsing the tool output.
fn item_completed_bodies(params: &Value) -> Vec<EngineEventBody> {
    let Some(item) = params.get("item") else {
        return Vec::new();
    };
    let Some(call_id) = item.get("id").and_then(Value::as_str) else {
        return Vec::new();
    };
    let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "commandExecution" => {
            let output = item.get("aggregatedOutput").cloned().unwrap_or(Value::Null);
            vec![EngineEventBody::ToolCallOutput {
                call_id: call_id.to_owned(),
                output,
                delta: false,
            }]
        }
        "fileChange" => file_change_bodies(call_id, item),
        "mcpToolCall" => vec![EngineEventBody::ToolCallOutput {
            call_id: call_id.to_owned(),
            output: item.get("result").cloned().unwrap_or(Value::Null),
            delta: false,
        }],
        "webSearch" => vec![EngineEventBody::ToolCallOutput {
            call_id: call_id.to_owned(),
            output: item.get("results").cloned().unwrap_or(Value::Null),
            delta: false,
        }],
        _ => Vec::new(),
    }
}

fn file_change_bodies(call_id: &str, item: &Value) -> Vec<EngineEventBody> {
    let mut bodies = vec![EngineEventBody::ToolCallOutput {
        call_id: call_id.to_owned(),
        output: item.get("changes").cloned().unwrap_or(Value::Null),
        delta: false,
    }];
    if let Some(changes) = item.get("changes").and_then(Value::as_array) {
        for change in changes {
            let Some(path) = change.get("path").and_then(Value::as_str) else {
                continue;
            };
            let kind = change_kind(change.get("kind").and_then(Value::as_str).unwrap_or(""));
            let diff = change
                .get("diff")
                .and_then(Value::as_str)
                .map(str::to_owned);
            bodies.push(EngineEventBody::FileChanged {
                path: path.to_owned(),
                change: kind,
                diff,
            });
        }
    }
    bodies
}

fn change_kind(value: &str) -> ChangeKind {
    match value {
        "add" | "added" | "created" => ChangeKind::Created,
        "delete" | "deleted" | "removed" => ChangeKind::Deleted,
        _ => ChangeKind::Modified,
    }
}

/// A codex `server/request` notification carries the request id and the
/// codex method that triggered it. Command/patch approvals map to
/// [`EngineEventBody::ApprovalRequest`]; a dynamic tool call maps to
/// [`EngineEventBody::ToolCallRequest`].
fn server_request_bodies(params: &Value) -> Vec<EngineEventBody> {
    let Some(req_id) = params.get("requestId").and_then(Value::as_i64) else {
        return Vec::new();
    };
    let method = params.get("method").and_then(Value::as_str).unwrap_or("");
    let inner = params.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "execCommandApproval" | "applyPatchApproval" => {
            let kind = if method == "applyPatchApproval" {
                crate::engine::ApprovalKind::FileChange
            } else {
                crate::engine::ApprovalKind::CommandExecution
            };
            vec![EngineEventBody::ApprovalRequest {
                req_id,
                kind,
                detail: inner,
            }]
        }
        // Dynamic tool the host must execute on codex's behalf.
        "tool/call" | "dynamicTool/call" => vec![EngineEventBody::ToolCallRequest {
            req_id,
            name: inner
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            args: inner.get("arguments").cloned().unwrap_or(Value::Null),
        }],
        _ => Vec::new(),
    }
}

fn token_usage(params: &Value) -> Option<crate::engine::Usage> {
    let usage = params.get("usage").or(Some(params))?;
    let read = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    Some(crate::engine::Usage {
        tokens_in: read("inputTokens"),
        tokens_out: read("outputTokens"),
        cache_read: read("cachedInputTokens"),
        cache_creation: read("cacheCreationTokens"),
        cost_usd: usage.get("costUsd").and_then(Value::as_f64),
    })
}

fn status_changed(params: &Value) -> Option<TurnStatus> {
    let status = params.get("status").and_then(Value::as_str)?;
    Some(match status {
        "idle" => TurnStatus::Idle,
        "running" | "active" => TurnStatus::Running,
        "blocked" | "awaitingInput" => TurnStatus::AwaitingInput,
        "errored" => TurnStatus::Errored,
        "cancelled" => TurnStatus::Cancelled,
        _ => return None,
    })
}

fn text_delta(params: &Value) -> Option<String> {
    params
        .get("delta")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn command_output_delta(params: &Value) -> Option<(String, String)> {
    let call_id = params
        .get("itemId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let delta = params
        .get("delta")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    Some((call_id.to_owned(), delta.to_owned()))
}

fn thread_id(params: &Value) -> Option<&str> {
    params
        .get("threadId")
        .or_else(|| params.get("thread").and_then(|t| t.get("threadId")))
        .or_else(|| params.get("turn").and_then(|t| t.get("threadId")))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ApprovalKind;

    fn note(method: &str, params: Value) -> Notification {
        Notification {
            method: method.to_owned(),
            params,
        }
    }

    fn map(note: &Notification) -> Vec<EngineEventBody> {
        let seq = AtomicU64::new(0);
        notification_to_events(note, &seq)
            .into_iter()
            .map(|e| e.body)
            .collect()
    }

    #[test]
    fn turn_started_maps_to_turn_started() {
        let bodies = map(&note(
            "turn/started",
            json!({ "threadId": "thread-1", "turnId": "turn-1" }),
        ));
        assert_eq!(
            bodies,
            vec![EngineEventBody::TurnStarted {
                thread_id: "thread-1".to_owned()
            }]
        );
    }

    #[test]
    fn agent_message_delta_maps_to_text_delta() {
        let bodies = map(&note(
            "item/agentMessage/delta",
            json!({ "threadId": "t", "itemId": "i", "delta": "hello" }),
        ));
        assert_eq!(
            bodies,
            vec![EngineEventBody::TextDelta {
                text: "hello".to_owned()
            }]
        );
    }

    #[test]
    fn reasoning_delta_maps_to_reasoning_delta() {
        let bodies = map(&note(
            "item/reasoning/textDelta",
            json!({ "threadId": "t", "itemId": "i", "delta": "thinking" }),
        ));
        assert_eq!(
            bodies,
            vec![EngineEventBody::ReasoningDelta {
                text: "thinking".to_owned()
            }]
        );
    }

    #[test]
    fn command_execution_started_and_completed() {
        let started = map(&note(
            "item/started",
            json!({
                "threadId": "t",
                "item": { "id": "c1", "type": "commandExecution", "command": "ls", "cwd": "/w" }
            }),
        ));
        assert_eq!(
            started,
            vec![EngineEventBody::ToolCallStarted {
                call_id: "c1".to_owned(),
                name: "shell".to_owned(),
                args: json!({ "command": "ls", "cwd": "/w" }),
            }]
        );

        let completed = map(&note(
            "item/completed",
            json!({
                "threadId": "t",
                "item": { "id": "c1", "type": "commandExecution", "aggregatedOutput": "file.txt\n" }
            }),
        ));
        assert_eq!(
            completed,
            vec![EngineEventBody::ToolCallOutput {
                call_id: "c1".to_owned(),
                output: json!("file.txt\n"),
                delta: false,
            }]
        );
    }

    #[test]
    fn file_change_emits_output_and_per_path_file_changed() {
        let bodies = map(&note(
            "item/completed",
            json!({
                "threadId": "t",
                "item": {
                    "id": "f1",
                    "type": "fileChange",
                    "changes": [
                        { "path": "hello.txt", "kind": "add", "diff": "+FOO" }
                    ]
                }
            }),
        ));
        assert_eq!(bodies.len(), 2);
        assert!(matches!(
            &bodies[0],
            EngineEventBody::ToolCallOutput { call_id, .. } if call_id == "f1"
        ));
        assert_eq!(
            bodies[1],
            EngineEventBody::FileChanged {
                path: "hello.txt".to_owned(),
                change: ChangeKind::Created,
                diff: Some("+FOO".to_owned()),
            }
        );
    }

    #[test]
    fn token_usage_maps_to_usage() {
        let bodies = map(&note(
            "thread/tokenUsage/updated",
            json!({
                "threadId": "t",
                "usage": {
                    "inputTokens": 100,
                    "outputTokens": 50,
                    "cachedInputTokens": 40,
                    "cacheCreationTokens": 10,
                    "costUsd": 0.0123
                }
            }),
        ));
        assert_eq!(
            bodies,
            vec![EngineEventBody::Usage {
                usage: crate::engine::Usage {
                    tokens_in: 100,
                    tokens_out: 50,
                    cache_read: 40,
                    cache_creation: 10,
                    cost_usd: Some(0.0123),
                }
            }]
        );
    }

    #[test]
    fn turn_completed_failed_cancelled() {
        assert_eq!(
            map(&note("turn/completed", json!({ "threadId": "t" }))),
            vec![EngineEventBody::TurnCompleted {
                outcome: TurnOutcome::Ok
            }]
        );
        assert_eq!(
            map(&note(
                "turn/failed",
                json!({ "threadId": "t", "error": { "message": "boom" } })
            )),
            vec![EngineEventBody::TurnCompleted {
                outcome: TurnOutcome::Error {
                    message: "boom".to_owned()
                }
            }]
        );
        assert_eq!(
            map(&note("turn/cancelled", json!({ "threadId": "t" }))),
            vec![EngineEventBody::TurnCompleted {
                outcome: TurnOutcome::Cancelled
            }]
        );
    }

    #[test]
    fn server_request_command_approval_maps_to_approval_request() {
        let bodies = map(&note(
            "server/request",
            json!({
                "requestId": 7,
                "method": "execCommandApproval",
                "params": { "command": "rm -rf /" }
            }),
        ));
        assert_eq!(
            bodies,
            vec![EngineEventBody::ApprovalRequest {
                req_id: 7,
                kind: ApprovalKind::CommandExecution,
                detail: json!({ "command": "rm -rf /" }),
            }]
        );
    }

    #[test]
    fn seq_increments_across_events() {
        let seq = AtomicU64::new(0);
        let n = note(
            "item/completed",
            json!({
                "threadId": "t",
                "item": {
                    "id": "f1",
                    "type": "fileChange",
                    "changes": [{ "path": "a", "kind": "modify" }]
                }
            }),
        );
        let events = notification_to_events(&n, &seq);
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[1].seq, 1);
        assert_eq!(seq.load(Ordering::Relaxed), 2);
    }

    // --- Lowering tests ---

    fn req(permissions: Permissions, effort: Option<Effort>) -> TurnRequest {
        TurnRequest {
            engine: EngineKind::Codex,
            model: "gpt-5.3-codex".to_owned(),
            effort,
            permissions,
            cwd: "/workspace".to_owned(),
            prompt: "write FOO".to_owned(),
            tools: vec![],
            run_id: None,
            node_id: None,
        }
    }

    #[test]
    fn lowers_workspace_write_to_sandbox_and_on_request_approval() {
        let params = turn_start_params(
            "thread-1",
            &req(Permissions::WorkspaceWrite, Some(Effort::Medium)),
        );
        assert_eq!(
            params.get("sandboxPolicy"),
            Some(&json!({ "type": "workspaceWrite", "networkAccess": true }))
        );
        assert_eq!(params.get("approvalPolicy"), Some(&json!("on-request")));
        assert_eq!(params.get("effort"), Some(&json!("medium")));
        assert_eq!(params.get("model"), Some(&json!("gpt-5.3-codex")));
    }

    #[test]
    fn lowers_danger_full_access_to_never_approval() {
        let params = turn_start_params("thread-1", &req(Permissions::DangerFullAccess, None));
        assert_eq!(
            params.get("sandboxPolicy"),
            Some(&json!({ "type": "dangerFullAccess" }))
        );
        assert_eq!(params.get("approvalPolicy"), Some(&json!("never")));
        // No effort field when the envelope leaves it unset.
        assert!(params.get("effort").is_none());
    }

    #[test]
    fn lowers_read_only_sandbox() {
        let params =
            turn_start_params("thread-1", &req(Permissions::ReadOnly, Some(Effort::Xhigh)));
        assert_eq!(
            params.get("sandboxPolicy"),
            Some(&json!({ "type": "readOnly", "networkAccess": true }))
        );
        assert_eq!(params.get("effort"), Some(&json!("xhigh")));
    }
}
