//! Claude adapter for the [`Engine`] trait.
//!
//! Claude Code ships a headless mode that reads turns as stream-json on
//! stdin and writes events as stream-json on stdout:
//!
//! ```text
//! claude --print --input-format stream-json --output-format stream-json \
//!        --verbose --include-partial-messages \
//!        --dangerously-skip-permissions --model <model>
//! ```
//!
//! `--include-partial-messages` is what turns the otherwise
//! one-message-per-block output into the Anthropic streaming events
//! (`content_block_delta` etc.), which map onto [`EngineEventBody`]'s
//! delta variants. Without it Claude emits one `assistant` event per
//! completed block, which still maps (as a single non-incremental
//! delta) but loses streaming.
//!
//! Session mode: this adapter runs a COLD per-turn session. Each
//! [`Engine::start_turn`] spawns one `claude --print` process, feeds it
//! the prompt as a single stream-json user message, reads the event
//! stream to the terminal `result`, and lets the process exit. The plan
//! flags warm multi-turn (one long-lived stdin stream reusing prompt
//! cache) as a verify-early optimization; a cold session is the
//! documented acceptable baseline and is what ships here. The warm
//! design fits the same interface: `start_turn` would write to a
//! retained stdin instead of spawning, and the reader task would
//! demux by `session_id`. The cost is the ~40k cache-creation tokens
//! per cold spawn the plan measured, which `Usage` surfaces so callers
//! can see it.
//!
//! Claude self-executes its tools under `--dangerously-skip-permissions`,
//! so it is a subset producer: it emits `ToolCallStarted`/`ToolCallOutput`
//! and never `ApprovalRequest`/`ToolCallRequest`.
//!
//! `ANTHROPIC_API_KEY` is read from the process environment by the
//! `claude` binary. It is never placed on argv.

use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Mutex, broadcast, watch},
};

use crate::engine::{
    Effort, Engine, EngineAnswer, EngineEvent, EngineEventBody, EngineKind, Permissions, RequestId,
    TurnHandle, TurnOutcome, TurnRequest, TurnStatus, Usage,
};

/// One Claude adapter. Holds the event fan-out and the live-turn
/// bookkeeping; each turn spawns its own `claude` subprocess.
pub struct ClaudeEngine {
    events: broadcast::Sender<EngineEvent>,
    /// Tracks whether any turn is currently running, for `status` and
    /// `wait_for_exit`. A cold session has no persistent process, so
    /// `wait_for_exit` resolves when the last in-flight turn finishes.
    running: Arc<Mutex<u32>>,
    idle: watch::Receiver<bool>,
    idle_tx: watch::Sender<bool>,
}

impl ClaudeEngine {
    pub fn new() -> Arc<Self> {
        let (events, _) = broadcast::channel::<EngineEvent>(1024);
        let (idle_tx, idle) = watch::channel(true);
        Arc::new(Self {
            events,
            running: Arc::new(Mutex::new(0)),
            idle,
            idle_tx,
        })
    }

    fn claude_bin() -> String {
        std::env::var("ROOM_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_owned())
    }
}

impl Engine for ClaudeEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Claude
    }

    async fn start_turn(&self, turn: TurnRequest) -> Result<TurnHandle> {
        let mut args = claude_args(&turn);
        let bin = Self::claude_bin();

        let mut child = Command::new(&bin)
            .args(&args)
            .current_dir(if turn.cwd.is_empty() { "." } else { &turn.cwd })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn `{bin} {}`", args.join(" ")))?;
        args.clear();

        let mut stdin = child.stdin.take().context("claude stdin was not piped")?;
        let stdout = child.stdout.take().context("claude stdout was not piped")?;

        // The first stream-json message carries the prompt as a user
        // turn. Cold session: we close stdin right after so claude
        // runs the single turn and exits.
        let user_message = user_turn_message(&turn.prompt);
        stdin
            .write_all(format!("{user_message}\n").as_bytes())
            .await
            .context("write claude user turn")?;
        stdin.flush().await.context("flush claude stdin")?;
        drop(stdin);

        {
            let mut running = self.running.lock().await;
            *running += 1;
            let _ = self.idle_tx.send(false);
        }

        let events = self.events.clone();
        let running = self.running.clone();
        let idle_tx = self.idle_tx.clone();

        // The handle's thread_id is the claude session id. We do not
        // have it until the `system/init` event arrives, so we hand
        // back a provisional id and let the reader task re-stamp events
        // with the real session id once known. Callers correlate by
        // run_id/node_id on the wire; the provisional id keeps the
        // handle non-empty for the synchronous return.
        let provisional = format!(
            "claude-{}",
            turn.node_id.clone().unwrap_or_else(|| "turn".to_owned())
        );
        let handle_id = Arc::new(Mutex::new(provisional.clone()));
        let handle_for_task = handle_id.clone();

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            let seq = AtomicU64::new(0);
            let mut session_id = provisional.clone();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if line.trim().is_empty() => continue,
                    Ok(Some(line)) => {
                        let value: Value = match serde_json::from_str(&line) {
                            Ok(v) => v,
                            Err(err) => {
                                eprintln!("room: claude emitted non-JSON line ({err}): {line}");
                                continue;
                            }
                        };
                        if let Some(id) = value.get("session_id").and_then(Value::as_str)
                            && id != session_id
                        {
                            session_id = id.to_owned();
                            *handle_for_task.lock().await = session_id.clone();
                        }
                        for body in stream_json_to_bodies(&value) {
                            let _ = events.send(EngineEvent {
                                turn_id: session_id.clone(),
                                seq: seq.fetch_add(1, Ordering::Relaxed),
                                body,
                            });
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        eprintln!("room: claude stdout read error: {err}");
                        break;
                    }
                }
            }
            let _ = child.wait().await;
            let mut count = running.lock().await;
            *count = count.saturating_sub(1);
            if *count == 0 {
                let _ = idle_tx.send(true);
            }
        });

        Ok(TurnHandle {
            thread_id: handle_id.lock().await.clone(),
        })
    }

    fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    async fn answer(&self, _req_id: RequestId, _answer: EngineAnswer) -> Result<()> {
        // Claude self-executes its tools under
        // `--dangerously-skip-permissions`, so it never raises a
        // server-initiated request to answer. Reaching here means a
        // caller answered a request this engine never made.
        anyhow::bail!("claude engine does not raise approval or tool-call requests")
    }

    async fn interrupt(&self, _turn: &TurnHandle) -> Result<()> {
        // A cold session's process is killed on drop when its turn
        // ends. There is no persistent process to signal between
        // turns; a warm session would send an interrupt control
        // message on the retained stdin here.
        anyhow::bail!("claude cold session has no interruptible persistent turn")
    }

    async fn status(&self, _turn: &TurnHandle) -> Result<TurnStatus> {
        let running = *self.running.lock().await;
        Ok(if running > 0 {
            TurnStatus::Running
        } else {
            TurnStatus::Idle
        })
    }

    async fn wait_for_exit(&self) {
        // Resolves when no turn is in flight. For a cold session this
        // is the natural quiescence point a supervisor would respawn
        // against.
        let mut idle = self.idle.clone();
        while !*idle.borrow() {
            if idle.changed().await.is_err() {
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------
// Lowering: TurnRequest -> claude flags + input message
// ---------------------------------------------------------------------

/// Build the argv (minus the binary) for a [`TurnRequest`]. Exposed to
/// tests so the model/permission/effort lowering is checked without a
/// live claude binary.
pub fn claude_args(turn: &TurnRequest) -> Vec<String> {
    let mut args = vec![
        "--print".to_owned(),
        "--input-format".to_owned(),
        "stream-json".to_owned(),
        "--output-format".to_owned(),
        "stream-json".to_owned(),
        "--verbose".to_owned(),
        // Token-level deltas. Without it claude emits one assistant
        // event per completed block instead of content_block_delta.
        "--include-partial-messages".to_owned(),
    ];

    if !turn.model.is_empty() {
        args.push("--model".to_owned());
        args.push(turn.model.clone());
    }

    permission_args(turn.permissions, &mut args);

    if let Some(flag) = effort_flag(turn.effort) {
        args.push("--thinking".to_owned());
        args.push(flag);
    }

    args
}

/// Lower the engine-agnostic permission level to claude's native
/// flags. `danger_full_access` skips all permission prompts;
/// `workspace_write` and `read_only` use `--permission-mode` so claude
/// asks before mutating outside its allowed scope. claude cannot
/// self-execute writes under a read-only mode, which matches the
/// envelope's intent.
fn permission_args(permissions: Permissions, args: &mut Vec<String>) {
    match permissions {
        Permissions::DangerFullAccess => {
            args.push("--dangerously-skip-permissions".to_owned());
        }
        Permissions::WorkspaceWrite => {
            args.push("--permission-mode".to_owned());
            args.push("acceptEdits".to_owned());
        }
        Permissions::ReadOnly => {
            args.push("--permission-mode".to_owned());
            args.push("plan".to_owned());
        }
    }
}

/// Map the reasoning budget to claude's `--thinking` budget. claude
/// honors `none` (off) and a tier; the finer codex tiers collapse onto
/// claude's coarser ones. Returning `None` leaves claude on its default.
fn effort_flag(effort: Option<Effort>) -> Option<String> {
    match effort? {
        Effort::None => Some("none".to_owned()),
        Effort::Minimal | Effort::Low => Some("low".to_owned()),
        Effort::Medium => Some("medium".to_owned()),
        Effort::High | Effort::Xhigh => Some("high".to_owned()),
    }
}

/// The stream-json user-turn message claude reads on stdin.
fn user_turn_message(prompt: &str) -> String {
    json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{ "type": "text", "text": prompt }]
        }
    })
    .to_string()
}

// ---------------------------------------------------------------------
// Lifting: claude stream-json event -> EngineEvent bodies
// ---------------------------------------------------------------------

/// Translate one claude stream-json line into zero or more canonical
/// [`EngineEventBody`]s. Pure: reads the parsed JSON, emits bodies. The
/// reader task stamps `turn_id`/`seq`. This is the unit-tested seam.
pub fn stream_json_to_bodies(value: &Value) -> Vec<EngineEventBody> {
    match value.get("type").and_then(Value::as_str) {
        Some("system") => system_bodies(value),
        // `--include-partial-messages` emits Anthropic streaming events
        // nested under `stream_event` (with an outer `type:"stream_event"`)
        // or, in some builds, with the streaming `type` at the top level.
        Some("stream_event") => stream_event_bodies(value.get("event").unwrap_or(value)),
        Some("content_block_delta") | Some("content_block_start") => stream_event_bodies(value),
        // Full assistant message (no partial-messages, or a final
        // coalesced message): lift each content block.
        Some("assistant") => assistant_message_bodies(value),
        Some("result") => result_bodies(value),
        // Lifecycle/control lines that carry no engine event of their own.
        // Listed explicitly so the catch-all only fires for a genuinely
        // unmodeled type.
        Some("content_block_stop")
        | Some("message_start")
        | Some("message_delta")
        | Some("message_stop")
        | Some("ping")
        | Some("user") => Vec::new(),
        other => {
            // A stream-json line we do not model. Surface it once on stderr
            // so a claude format change shows up in journald instead of
            // being dropped without trace; the synchronous collector still
            // completes on the terminal `result` event.
            eprintln!("room-server: claude stream-json: unhandled event type {other:?}");
            Vec::new()
        }
    }
}

fn system_bodies(value: &Value) -> Vec<EngineEventBody> {
    if value.get("subtype").and_then(Value::as_str) == Some("init")
        && let Some(session_id) = value.get("session_id").and_then(Value::as_str)
    {
        return vec![EngineEventBody::TurnStarted {
            thread_id: session_id.to_owned(),
        }];
    }
    Vec::new()
}

/// Anthropic streaming events: `content_block_start` (carries a
/// `tool_use` block's id/name/input) and `content_block_delta` (text,
/// thinking, or tool input json deltas).
fn stream_event_bodies(event: &Value) -> Vec<EngineEventBody> {
    match event.get("type").and_then(Value::as_str) {
        Some("content_block_start") => {
            let block = event.get("content_block").unwrap_or(&Value::Null);
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                return vec![EngineEventBody::ToolCallStarted {
                    call_id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    args: block.get("input").cloned().unwrap_or(json!({})),
                }];
            }
            Vec::new()
        }
        Some("content_block_delta") => {
            let delta = event.get("delta").unwrap_or(&Value::Null);
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => delta
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|text| {
                        vec![EngineEventBody::TextDelta {
                            text: text.to_owned(),
                        }]
                    })
                    .unwrap_or_default(),
                Some("thinking_delta") => delta
                    .get("thinking")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(|text| {
                        vec![EngineEventBody::ReasoningDelta {
                            text: text.to_owned(),
                        }]
                    })
                    .unwrap_or_default(),
                // Incremental tool-argument JSON. The complete tool_use with
                // full args arrives in the coalesced `assistant` message
                // (assistant_message_bodies), so the synchronous collector
                // does not need the partial fragments. Re-emitting them as
                // incremental ToolCallOutput would require tracking the
                // content-block index to the tool-call id across lines.
                // Handled explicitly so it is a documented no-op, not a
                // silent drop.
                Some("input_json_delta") => Vec::new(),
                other => {
                    eprintln!(
                        "room-server: claude stream-json: unhandled content_block_delta {other:?}"
                    );
                    Vec::new()
                }
            }
        }
        _ => Vec::new(),
    }
}

/// A complete `assistant` message event. Used when partial messages are
/// off, or for the coalesced final message. Each content block becomes
/// its own body.
fn assistant_message_bodies(value: &Value) -> Vec<EngineEventBody> {
    let Some(content) = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let mut bodies = Vec::new();
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    bodies.push(EngineEventBody::TextDelta {
                        text: text.to_owned(),
                    });
                }
            }
            Some("thinking") => {
                if let Some(text) = block.get("thinking").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    bodies.push(EngineEventBody::ReasoningDelta {
                        text: text.to_owned(),
                    });
                }
            }
            Some("tool_use") => {
                bodies.push(EngineEventBody::ToolCallStarted {
                    call_id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    args: block.get("input").cloned().unwrap_or(json!({})),
                });
            }
            // A `tool_result` block (claude self-executed a tool and is
            // reporting the output) maps to ToolCallOutput.
            Some("tool_result") => {
                bodies.push(EngineEventBody::ToolCallOutput {
                    call_id: block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    output: block.get("content").cloned().unwrap_or(Value::Null),
                    delta: false,
                });
            }
            _ => {}
        }
    }
    bodies
}

/// The terminal `result` event. Carries usage/cost and the final
/// outcome. Emits a `Usage` then a `TurnCompleted`.
fn result_bodies(value: &Value) -> Vec<EngineEventBody> {
    let usage = usage_from_result(value);
    let is_error = value
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || value.get("subtype").and_then(Value::as_str) == Some("error_during_execution");
    let outcome = if is_error {
        let message = value
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or("claude turn failed")
            .to_owned();
        TurnOutcome::Error { message }
    } else {
        TurnOutcome::Ok
    };
    vec![
        EngineEventBody::Usage { usage },
        EngineEventBody::TurnCompleted { outcome },
    ]
}

fn usage_from_result(value: &Value) -> Usage {
    let usage = value.get("usage").cloned().unwrap_or(Value::Null);
    let read = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    Usage {
        tokens_in: read("input_tokens"),
        tokens_out: read("output_tokens"),
        cache_read: read("cache_read_input_tokens"),
        cache_creation: read("cache_creation_input_tokens"),
        cost_usd: value.get("total_cost_usd").and_then(Value::as_f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(permissions: Permissions, effort: Option<Effort>, model: &str) -> TurnRequest {
        TurnRequest {
            engine: EngineKind::Claude,
            model: model.to_owned(),
            effort,
            permissions,
            cwd: "/workspace".to_owned(),
            prompt: "write FOO".to_owned(),
            tools: vec![],
            run_id: None,
            node_id: None,
        }
    }

    // --- Lowering tests ---

    #[test]
    fn danger_full_access_skips_permissions() {
        let args = claude_args(&req(Permissions::DangerFullAccess, None, "haiku"));
        assert!(args.contains(&"--dangerously-skip-permissions".to_owned()));
        assert!(!args.contains(&"--permission-mode".to_owned()));
        assert!(args.windows(2).any(|w| w == ["--model", "haiku"]));
    }

    #[test]
    fn workspace_write_uses_accept_edits_mode() {
        let args = claude_args(&req(Permissions::WorkspaceWrite, None, "sonnet"));
        assert!(
            args.windows(2)
                .any(|w| w == ["--permission-mode", "acceptEdits"])
        );
        assert!(!args.contains(&"--dangerously-skip-permissions".to_owned()));
    }

    #[test]
    fn read_only_uses_plan_mode() {
        let args = claude_args(&req(Permissions::ReadOnly, None, "opus"));
        assert!(args.windows(2).any(|w| w == ["--permission-mode", "plan"]));
    }

    #[test]
    fn effort_lowers_to_thinking_budget() {
        let high = claude_args(&req(
            Permissions::DangerFullAccess,
            Some(Effort::Xhigh),
            "opus",
        ));
        assert!(high.windows(2).any(|w| w == ["--thinking", "high"]));
        let none = claude_args(&req(
            Permissions::DangerFullAccess,
            Some(Effort::None),
            "opus",
        ));
        assert!(none.windows(2).any(|w| w == ["--thinking", "none"]));
        let unset = claude_args(&req(Permissions::DangerFullAccess, None, "opus"));
        assert!(!unset.contains(&"--thinking".to_owned()));
    }

    #[test]
    fn always_streams_with_partial_messages() {
        let args = claude_args(&req(Permissions::DangerFullAccess, None, "haiku"));
        assert!(args.contains(&"--include-partial-messages".to_owned()));
        assert!(
            args.windows(2)
                .any(|w| w == ["--input-format", "stream-json"])
        );
        assert!(
            args.windows(2)
                .any(|w| w == ["--output-format", "stream-json"])
        );
    }

    // --- stream-json mapping tests ---

    #[test]
    fn input_json_delta_is_a_documented_noop() {
        // Incremental tool-argument fragments produce no incremental body;
        // the complete tool_use arrives via the coalesced assistant message.
        let line = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "input_json_delta", "partial_json": "{\"path\":" }
        });
        assert!(stream_json_to_bodies(&line).is_empty());
    }

    #[test]
    fn assistant_tool_use_yields_full_args() {
        // The completeness guarantee that lets input_json_delta be a no-op:
        // the coalesced assistant message carries the whole tool call.
        let line = json!({
            "type": "assistant",
            "message": { "content": [
                { "type": "tool_use", "id": "toolu_1", "name": "bash",
                  "input": { "command": "ls -a" } }
            ] }
        });
        let bodies = stream_json_to_bodies(&line);
        assert_eq!(bodies.len(), 1);
        match &bodies[0] {
            EngineEventBody::ToolCallStarted {
                call_id,
                name,
                args,
            } => {
                assert_eq!(call_id, "toolu_1");
                assert_eq!(name, "bash");
                assert_eq!(args, &json!({ "command": "ls -a" }));
            }
            other => panic!("expected ToolCallStarted, got {other:?}"),
        }
    }

    #[test]
    fn unmodeled_control_lines_drop_without_body() {
        for kind in [
            "message_start",
            "message_delta",
            "content_block_stop",
            "ping",
        ] {
            let line = json!({ "type": kind });
            assert!(
                stream_json_to_bodies(&line).is_empty(),
                "expected {kind} to produce no body"
            );
        }
    }

    // --- Lifting tests ---

    #[test]
    fn system_init_maps_to_turn_started() {
        let value = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "sess-1",
            "model": "claude-haiku"
        });
        assert_eq!(
            stream_json_to_bodies(&value),
            vec![EngineEventBody::TurnStarted {
                thread_id: "sess-1".to_owned()
            }]
        );
    }

    #[test]
    fn text_delta_maps_to_text_delta() {
        let value = json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "delta": { "type": "text_delta", "text": "hel" }
            }
        });
        assert_eq!(
            stream_json_to_bodies(&value),
            vec![EngineEventBody::TextDelta {
                text: "hel".to_owned()
            }]
        );
    }

    #[test]
    fn thinking_delta_maps_to_reasoning_delta() {
        let value = json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "delta": { "type": "thinking_delta", "thinking": "let me" }
            }
        });
        assert_eq!(
            stream_json_to_bodies(&value),
            vec![EngineEventBody::ReasoningDelta {
                text: "let me".to_owned()
            }]
        );
    }

    #[test]
    fn tool_use_block_start_maps_to_tool_call_started() {
        let value = json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_start",
                "content_block": {
                    "type": "tool_use",
                    "id": "toolu_1",
                    "name": "Bash",
                    "input": { "command": "ls" }
                }
            }
        });
        assert_eq!(
            stream_json_to_bodies(&value),
            vec![EngineEventBody::ToolCallStarted {
                call_id: "toolu_1".to_owned(),
                name: "Bash".to_owned(),
                args: json!({ "command": "ls" }),
            }]
        );
    }

    #[test]
    fn full_assistant_message_lifts_each_block() {
        let value = json!({
            "type": "assistant",
            "session_id": "sess-1",
            "message": {
                "role": "assistant",
                "content": [
                    { "type": "text", "text": "done" },
                    { "type": "tool_use", "id": "toolu_2", "name": "Write", "input": { "path": "hello.txt" } }
                ]
            }
        });
        assert_eq!(
            stream_json_to_bodies(&value),
            vec![
                EngineEventBody::TextDelta {
                    text: "done".to_owned()
                },
                EngineEventBody::ToolCallStarted {
                    call_id: "toolu_2".to_owned(),
                    name: "Write".to_owned(),
                    args: json!({ "path": "hello.txt" }),
                },
            ]
        );
    }

    #[test]
    fn result_emits_usage_then_completed() {
        let value = json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "wrote FOO",
            "session_id": "sess-1",
            "total_cost_usd": 0.0042,
            "usage": {
                "input_tokens": 12,
                "output_tokens": 8,
                "cache_read_input_tokens": 40000,
                "cache_creation_input_tokens": 100
            }
        });
        assert_eq!(
            stream_json_to_bodies(&value),
            vec![
                EngineEventBody::Usage {
                    usage: Usage {
                        tokens_in: 12,
                        tokens_out: 8,
                        cache_read: 40000,
                        cache_creation: 100,
                        cost_usd: Some(0.0042),
                    }
                },
                EngineEventBody::TurnCompleted {
                    outcome: TurnOutcome::Ok
                },
            ]
        );
    }

    #[test]
    fn result_error_maps_to_error_outcome() {
        let value = json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "result": "boom",
            "session_id": "sess-1",
            "total_cost_usd": 0.0
        });
        let bodies = stream_json_to_bodies(&value);
        assert_eq!(
            bodies.last(),
            Some(&EngineEventBody::TurnCompleted {
                outcome: TurnOutcome::Error {
                    message: "boom".to_owned()
                }
            })
        );
    }
}
