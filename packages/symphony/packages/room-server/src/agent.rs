//! Engine-agnostic agent surface.
//!
//! This is the engine-agnostic turn path. It looks an engine up in the
//! [`EngineRegistry`] by the `engine` field on the incoming
//! [`TurnRequest`], starts the turn, and drives the canonical
//! [`EngineEvent`] stream. It never names a concrete engine.
//!
//! Unlike the chat path, a turn submitted here opens its own engine
//! thread, so nothing seeds the room thread row first. We seed it and
//! record the user prompt as the turn starts, let the codex bridge
//! stream the assistant/tool items into that same thread, and flip the
//! thread to its terminal status when the turn completes. Without this a
//! Symphony run's turn ran but left no transcript the room UI could
//! read.

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use ulid::Ulid;

use crate::{
    codex_bridge::Delta,
    db::{Message, ThreadUpsert, derive_preview, derive_title},
    engine::{Effort, EngineEventBody, TurnOutcome, TurnRequest, Usage},
    engine_handle::EngineHandle,
    state::AppState,
};

/// Subscribe, start the turn, record the prompt, then collect every
/// [`EngineEvent`] up to and including its `TurnCompleted`, persisting a
/// transcript as it goes.
///
/// The subscribe-before-start ordering is load-bearing: starting first
/// would race the first events (`turnStarted`, early deltas) onto the
/// broadcast before the receiver is attached. One engine instance runs
/// one turn at a time today, so the collector accepts the whole stream
/// and stops at the first `TurnCompleted`.
async fn run_turn_collect(
    state: &AppState,
    engine: &EngineHandle,
    turn: TurnRequest,
) -> anyhow::Result<AgentTurnResponse> {
    let now = now_ms();
    // Hold the prompt/metadata for the thread row before `turn` moves
    // into `start_turn`.
    let prompt = turn.prompt.clone();
    let model = (!turn.model.is_empty()).then(|| turn.model.clone());
    let cwd = (!turn.cwd.is_empty()).then(|| turn.cwd.clone());
    let effort = effort_label(turn.effort);
    let run_id = turn.run_id.clone();
    let node_id = turn.node_id.clone();

    let mut rx = engine.subscribe();
    let handle = engine.start_turn(turn).await?;
    let thread_id = handle.thread_id;

    if let Err(err) = record_turn_open(
        state, &thread_id, &prompt, model, cwd, effort, run_id, node_id, now,
    )
    .await
    {
        // Best-effort: a transcript missing its prompt row is better than
        // failing the whole turn, which the runtime would record as a node
        // failure.
        eprintln!("room: agent turn failed to record prompt: {err:#}");
    }

    let mut collected = Vec::new();
    loop {
        match rx.recv().await {
            Ok(event) => {
                let terminal = matches!(event.body, EngineEventBody::TurnCompleted { .. });
                collected.push(event);
                if terminal {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }

    let outcome = collected
        .iter()
        .rev()
        .find_map(|e| match &e.body {
            EngineEventBody::TurnCompleted { outcome } => Some(outcome.clone()),
            _ => None,
        })
        .unwrap_or(TurnOutcome::Ok);
    // The last Usage event holds the cumulative whole-turn total; earlier
    // ones are running subtotals codex re-emits as the turn progresses.
    let usage = collected
        .iter()
        .rev()
        .find_map(|e| match &e.body {
            EngineEventBody::Usage { usage } => Some(usage.clone()),
            _ => None,
        })
        .unwrap_or_default();

    record_turn_close(state, &thread_id, &outcome, now_ms()).await;

    Ok(AgentTurnResponse {
        thread_id,
        event_count: collected.len(),
        outcome,
        usage,
    })
}

/// Lowercase label for codex's `ReasoningEffort`, matching the strings
/// the thread row's `reasoning_effort` column carries on the chat path.
fn effort_label(effort: Option<Effort>) -> Option<String> {
    effort.map(|e| {
        match e {
            Effort::None => "none",
            Effort::Minimal => "minimal",
            Effort::Low => "low",
            Effort::Medium => "medium",
            Effort::High => "high",
            Effort::Xhigh => "xhigh",
        }
        .to_owned()
    })
}

/// Seed the thread row and record the user prompt for an engine-driven
/// turn, broadcasting both so a live viewer sees the row appear. `user`
/// is the run id (so a Symphony run's threads group under one author)
/// and the prompt author carries the node id.
#[allow(clippy::too_many_arguments)]
async fn record_turn_open(
    state: &AppState,
    thread_id: &str,
    prompt: &str,
    model: Option<String>,
    cwd: Option<String>,
    reasoning_effort: Option<String>,
    run_id: Option<String>,
    node_id: Option<String>,
    now: i64,
) -> anyhow::Result<()> {
    let user = run_id.unwrap_or_else(|| "symphony".to_owned());
    let author = match node_id {
        Some(node) => format!("symphony:{node}"),
        None => "symphony".to_owned(),
    };
    let message = Message {
        id: Ulid::new().to_string(),
        thread_id: thread_id.to_owned(),
        ts_ms: now,
        role: "user".to_owned(),
        kind: "user_prompt".to_owned(),
        text: Some(prompt.to_owned()),
        tool_name: Some(author),
        tool_use_id: None,
        tool_input: None,
        result: None,
        patch: None,
        images: Vec::new(),
    };

    let (thread, message) = {
        let db = state.db.lock().await;
        let thread = db.upsert_thread(&ThreadUpsert {
            id: thread_id.to_owned(),
            user,
            host: String::new(),
            repo: None,
            branch: None,
            cwd,
            workspace_root: None,
            base_sha: None,
            model,
            reasoning_effort,
            approval_policy: None,
            permission_profile: None,
            title_if_empty: Some(derive_title(prompt)),
            status: Some("active".to_owned()),
            now_ms: now,
            preview: Some(derive_preview(prompt)),
        })?;
        let message = db.insert_message(&message)?;
        (thread, message)
    };

    let _ = state.broadcast.send(Delta::MessageAppend {
        thread_id: thread_id.to_owned(),
        message,
    });
    let _ = state.broadcast.send(Delta::ThreadUpsert { thread });
    Ok(())
}

/// Flip the thread to its terminal status once the turn completes. The
/// codex bridge also lands an `idle` on `turn/completed`; this is the
/// engine-agnostic write that also covers an error/cancel outcome and a
/// self-executing engine with no bridge.
async fn record_turn_close(state: &AppState, thread_id: &str, outcome: &TurnOutcome, now: i64) {
    let status = match outcome {
        TurnOutcome::Ok => "idle",
        TurnOutcome::Error { .. } => "errored",
        TurnOutcome::Cancelled => "cancelled",
    };
    match state
        .db
        .lock()
        .await
        .set_thread_status(thread_id, status, now)
    {
        Ok(Some(thread)) => {
            let _ = state.broadcast.send(Delta::ThreadUpsert { thread });
        }
        Ok(None) => {}
        Err(err) => eprintln!("room: agent turn failed to set terminal status: {err:#}"),
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Summary returned by the engine-agnostic turn endpoint. The endpoint
/// awaits the whole turn and reports the terminal outcome plus the
/// thread id the engine assigned, so a caller that does not yet consume
/// the streaming surface still gets a usable result.
///
/// `usage` carries the turn's terminal token/cost totals. Both engines
/// emit cumulative [`Usage`] events (codex on `thread/tokenUsage/updated`,
/// claude once on the final result), so the last `Usage` event in the
/// stream is the whole-turn total. Without it the synchronous response
/// drops cost on the floor and an IR run can never show a non-nil cost
/// (the streaming surface that would otherwise carry per-event deltas is
/// deferred). Defaulted so a turn that emitted no usage still serializes
/// a zeroed total rather than omitting the field.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurnResponse {
    pub thread_id: String,
    pub outcome: TurnOutcome,
    pub event_count: usize,
    pub usage: Usage,
}

/// `POST /api/agent/turns`: run a turn through whichever engine the
/// [`TurnRequest`] names. Engine-agnostic: it never mentions codex or
/// claude, it dispatches on the request's `engine` field.
pub async fn agent_turn(
    State(state): State<AppState>,
    Json(turn): Json<TurnRequest>,
) -> impl IntoResponse {
    let Some(engine) = state.engines.get(turn.engine).cloned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("engine {:?} not configured on this server", turn.engine),
        )
            .into_response();
    };

    match run_turn_collect(&state, &engine, turn).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}")).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    use anyhow::Result;
    use tokio::sync::broadcast;

    use super::*;
    use crate::engine::{
        Engine, EngineAnswer, EngineEvent, EngineKind, Permissions, RequestId, TurnHandle,
        TurnStatus, Usage,
    };

    /// A fake engine that replays a scripted set of event bodies for one
    /// turn. Proves a turn flows through the [`Engine`] interface and the
    /// collector terminates on `TurnCompleted` without any subprocess.
    struct FakeEngine {
        events: broadcast::Sender<EngineEvent>,
        script: Vec<EngineEventBody>,
    }

    impl FakeEngine {
        fn new(script: Vec<EngineEventBody>) -> Arc<Self> {
            let (events, _) = broadcast::channel(64);
            Arc::new(Self { events, script })
        }
    }

    impl Engine for FakeEngine {
        fn kind(&self) -> EngineKind {
            EngineKind::Codex
        }

        async fn start_turn(&self, _turn: TurnRequest) -> Result<TurnHandle> {
            let events = self.events.clone();
            let script = self.script.clone();
            tokio::spawn(async move {
                let seq = AtomicU64::new(0);
                // Yield so the caller's subscribe + the receiver attach
                // before the first send, mirroring the real ordering
                // guarantee the collector relies on.
                tokio::task::yield_now().await;
                for body in script {
                    let _ = events.send(EngineEvent {
                        turn_id: "fake-thread".to_owned(),
                        seq: seq.fetch_add(1, Ordering::Relaxed),
                        body,
                    });
                }
            });
            Ok(TurnHandle {
                thread_id: "fake-thread".to_owned(),
            })
        }

        fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
            self.events.subscribe()
        }

        async fn answer(&self, _req_id: RequestId, _answer: EngineAnswer) -> Result<()> {
            Ok(())
        }

        async fn interrupt(&self, _turn: &TurnHandle) -> Result<()> {
            Ok(())
        }

        async fn status(&self, _turn: &TurnHandle) -> Result<TurnStatus> {
            Ok(TurnStatus::Idle)
        }

        async fn wait_for_exit(&self) {}
    }

    fn turn() -> TurnRequest {
        TurnRequest {
            engine: EngineKind::Codex,
            model: "gpt-5.3-codex".to_owned(),
            effort: None,
            permissions: Permissions::WorkspaceWrite,
            cwd: "/workspace".to_owned(),
            prompt: "write FOO".to_owned(),
            tools: vec![],
            run_id: Some("run_x".to_owned()),
            node_id: Some("n0".to_owned()),
        }
    }

    #[tokio::test]
    async fn collects_turn_through_the_trait_until_completed() {
        let engine = FakeEngine::new(vec![
            EngineEventBody::TurnStarted {
                thread_id: "fake-thread".to_owned(),
            },
            EngineEventBody::TextDelta {
                text: "FOO".to_owned(),
            },
            EngineEventBody::Usage {
                usage: Usage {
                    tokens_in: 10,
                    tokens_out: 2,
                    ..Default::default()
                },
            },
            EngineEventBody::TurnCompleted {
                outcome: TurnOutcome::Ok,
            },
        ]);

        // Drive through a trait object boundary equivalent: call the
        // generic collector against the trait directly.
        let mut rx = engine.subscribe();
        let handle = engine.start_turn(turn()).await.unwrap();
        let mut collected = Vec::new();
        loop {
            let event = rx.recv().await.unwrap();
            let terminal = matches!(event.body, EngineEventBody::TurnCompleted { .. });
            collected.push(event);
            if terminal {
                break;
            }
        }

        assert_eq!(handle.thread_id, "fake-thread");
        assert_eq!(collected.len(), 4);
        assert!(matches!(
            collected.last().unwrap().body,
            EngineEventBody::TurnCompleted {
                outcome: TurnOutcome::Ok
            }
        ));
    }
}
