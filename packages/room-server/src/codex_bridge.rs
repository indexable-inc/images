// Translates codex app-server notifications into room DB writes and
// Delta broadcasts.
//
// The bridge runs as one tokio task per `CodexClient`. It subscribes
// to every notification and routes each interesting one into the same
// SQLite + broadcast surface that the hook ingest path uses, so the
// rest of the server (WS, HTTP) stays unaware that the data came from
// a locally-driven codex turn rather than a remote machine's hook.
//
// Three notification streams drive the bridge, all converging on a
// single `flush` step that writes the buffer's current state to the
// DB and broadcasts the resulting delta:
//
//   - `item/started` — placeholder write. We seed an `ItemBuffer`
//     from the partial payload (kind, tool name, command line) so
//     the UI sees a row immediately.
//   - Deltas (`item/agentMessage/delta`, `item/reasoning/textDelta`,
//     `item/reasoning/summaryTextDelta`,
//     `item/commandExecution/outputDelta`) — append to the buffer
//     and flush. Each delta rewrites the same row so the UI streams
//     content as it arrives instead of waiting for the final event.
//   - `item/completed` — overlay the canonical fields from the full
//     item payload (which carries authoritative text, exit codes,
//     tool results, patches) onto the buffer, flush one last time,
//     drop the buffer.
//
// `turn/completed` clears any leftover buffers for the thread and
// flips its status back to idle. `turn/plan/updated` carries the
// agent's evolving TODO list; we persist it on the thread row and
// re-broadcast as a `ThreadUpsert` so the UI can render a checklist.
// Other notifications (token usage, account events) are dropped
// because the UI has nowhere to render them yet.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result};
use loro::{ExportMode, LoroDoc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, broadcast};
use ulid::Ulid;

use crate::{
    codex_rpc::{CodexClient, Notification},
    db::{
        Db, Message, PlanStep, Thread, ThreadGoal, ThreadPlan, ThreadUpsert, derive_preview,
        derive_title,
    },
    tool_result::ToolResult,
    workspace,
};

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Delta {
    ThreadUpsert { thread: Thread },
    MessageAppend { thread_id: String, message: Message },
    MessageUpdate { thread_id: String, message: Message },
    ThreadArchive { thread_id: String },
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnOptions {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub approval_policy: Option<Value>,
    pub permission_profile: Option<String>,
    pub sandbox: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub dynamic_tools: Vec<Value>,
    pub summary: Option<String>,
}

// ---------------------------------------------------------------------
// Buffer + kind
// ---------------------------------------------------------------------

/// Per-item streaming state. Owned exclusively by the bridge task —
/// no cross-thread synchronization needed.
#[derive(Default)]
struct ItemBuffer {
    thread_id: String,
    kind: ItemKind,
    /// Accumulated agentMessage text or reasoning content stream.
    text: String,
    /// Accumulated reasoning summary stream.
    summary: String,
    /// Accumulated stdout/stderr for commandExecution.
    command_output: String,
    /// Tool-call input (command + cwd, file changes, mcp args, search query).
    tool_input: Option<Value>,
    tool_name: Option<String>,
    /// Typed outcome, set from `item/completed` for tool kinds (and from
    /// codex `status`). `None` until we have a terminal payload, so the
    /// row renders as `Running` while in flight.
    result: Option<ToolResult>,
    /// Set on `item/completed` for fileChange (serialised diff).
    patch: Option<String>,
    started_at_ms: i64,
    /// Set on `item/completed`; takes precedence over `started_at_ms`
    /// when rendering so the row's ts reflects the canonical finish
    /// time.
    completed_at_ms: Option<i64>,
}

impl ItemBuffer {
    fn ts(&self) -> i64 {
        self.completed_at_ms.unwrap_or(self.started_at_ms)
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    #[default]
    Unknown,
    AgentMessage,
    Reasoning,
    CommandExecution,
    FileChange,
    McpToolCall,
    WebSearch,
}

impl ItemKind {
    fn parse(s: &str) -> Self {
        match s {
            "agentMessage" => Self::AgentMessage,
            "reasoning" => Self::Reasoning,
            "commandExecution" => Self::CommandExecution,
            "fileChange" => Self::FileChange,
            "mcpToolCall" => Self::McpToolCall,
            "webSearch" => Self::WebSearch,
            _ => Self::Unknown,
        }
    }
}

type Buffers = HashMap<String, ItemBuffer>;

// ---------------------------------------------------------------------
// Bridge entry point
// ---------------------------------------------------------------------

/// Wire up the bridge. Returns immediately; spawns a background task
/// that lives as long as the broadcast channels and codex subscriber.
pub fn start(
    client: Arc<CodexClient>,
    db: Arc<Mutex<Db>>,
    broadcast: broadcast::Sender<Delta>,
    loro_doc: Arc<Mutex<LoroDoc>>,
    loro_broadcast: broadcast::Sender<Vec<u8>>,
) {
    let mut rx = client.subscribe();
    tokio::spawn(async move {
        let mut buffers: Buffers = HashMap::new();
        loop {
            match rx.recv().await {
                Ok(note) => {
                    if let Err(err) =
                        record_codex_loro(&db, &loro_doc, &loro_broadcast, &note).await
                    {
                        eprintln!(
                            "room: failed to record codex {} in loro: {err:#}",
                            note.method
                        );
                    }
                    if let Err(err) = handle(&db, &broadcast, &mut buffers, &note).await {
                        eprintln!(
                            "room: codex bridge failed to handle {}: {err:#}",
                            note.method
                        );
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("room: codex bridge lagged, dropped {n} notifications");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        // The codex subprocess is gone. Any thread still 'active' or
        // 'blocked' will never receive another turn/completed, so the
        // UI would spin forever. Flip them back to 'idle' and
        // broadcast so connected clients clear their spinners.
        match db.lock().await.reset_stuck_threads() {
            Ok(threads) => {
                if !threads.is_empty() {
                    eprintln!(
                        "room: codex bridge closed; resetting {} stuck thread(s)",
                        threads.len()
                    );
                }
                for thread in threads {
                    let _ = broadcast.send(Delta::ThreadUpsert { thread });
                }
            }
            Err(err) => eprintln!("room: failed to reset stuck threads on bridge close: {err:#}"),
        }
    });
}

/// Persist every Codex app-server notification/request into the shared
/// Loro document before any lossy Room projection runs. The UI can
/// render known projections today and still inspect newly-added Codex
/// methods without a server release.
async fn record_codex_loro(
    db: &Arc<Mutex<Db>>,
    loro_doc: &Arc<Mutex<LoroDoc>>,
    loro_broadcast: &broadcast::Sender<Vec<u8>>,
    note: &Notification,
) -> Result<()> {
    let now = now_ms();
    let event_id = format!("{now:013}-{}", Ulid::new());
    let thread_id = note
        .params
        .get("threadId")
        .or_else(|| note.params.get("thread").and_then(|t| t.get("threadId")))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let turn_id = note
        .params
        .get("turnId")
        .or_else(|| note.params.get("turn").and_then(|t| t.get("id")))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let item_id = note
        .params
        .get("itemId")
        .or_else(|| note.params.get("item").and_then(|i| i.get("id")))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let record = json!({
        "id": event_id,
        "tsMs": now,
        "method": note.method,
        "threadId": thread_id,
        "turnId": turn_id,
        "itemId": item_id,
        "params": note.params,
    });

    let bytes = {
        let doc = loro_doc.lock().await;
        let before = doc.oplog_vv();
        doc.get_map("codex:events")
            .insert(&event_id, serde_json::to_string(&record)?)?;
        project_codex_loro(&doc, note, now)?;
        doc.commit();
        doc.export(ExportMode::updates(&before))?
    };

    if !bytes.is_empty() {
        {
            let db = db.lock().await;
            db.append_loro_update(now, &bytes)?;
        }
        let _ = loro_broadcast.send(bytes);
    }
    Ok(())
}

fn project_codex_loro(doc: &LoroDoc, note: &Notification, now: i64) -> Result<()> {
    match note.method.as_str() {
        "server/request" => {
            let request_id = note.params.get("requestId").map(value_key);
            if let Some(request_id) = request_id {
                doc.get_map("codex:requests").insert(
                    &request_id,
                    serde_json::to_string(&json!({
                        "requestId": request_id,
                        "method": note.params.get("method"),
                        "params": note.params.get("params"),
                        "status": "pending",
                        "tsMs": now,
                    }))?,
                )?;
            }
        }
        "serverRequest/resolved" => {
            if let Some(request_id) = note.params.get("requestId").map(value_key) {
                doc.get_map("codex:requests").insert(
                    &request_id,
                    serde_json::to_string(&json!({
                        "requestId": request_id,
                        "threadId": note.params.get("threadId"),
                        "status": "resolved",
                        "tsMs": now,
                    }))?,
                )?;
            }
        }
        "turn/started" | "turn/completed" => {
            if let Some(turn) = note.params.get("turn")
                && let Some(thread_id) = turn
                    .get("threadId")
                    .or_else(|| note.params.get("threadId"))
                    .and_then(Value::as_str)
            {
                doc.get_map("codex:turns")
                    .insert(thread_id, serde_json::to_string(turn)?)?;
            }
        }
        "thread/status/changed" => {
            if let Some(thread_id) = note.params.get("threadId").and_then(Value::as_str) {
                doc.get_map("codex:threadStatus")
                    .insert(thread_id, serde_json::to_string(&note.params)?)?;
            }
        }
        "thread/tokenUsage/updated" => {
            if let Some(thread_id) = note.params.get("threadId").and_then(Value::as_str) {
                doc.get_map("codex:tokenUsage")
                    .insert(thread_id, serde_json::to_string(&note.params)?)?;
            }
        }
        "turn/diff/updated" => {
            if let (Some(thread_id), Some(turn_id)) = (
                note.params.get("threadId").and_then(Value::as_str),
                note.params.get("turnId").and_then(Value::as_str),
            ) {
                doc.get_map("codex:diffs").insert(
                    &format!("{thread_id}:{turn_id}"),
                    serde_json::to_string(&note.params)?,
                )?;
            }
        }
        "error"
        | "warning"
        | "guardianWarning"
        | "configWarning"
        | "model/rerouted"
        | "model/verification"
        | "mcpServer/startupStatus/updated" => {
            doc.get_map("codex:diagnostics").insert(
                &format!("{now:013}-{}", Ulid::new()),
                serde_json::to_string(&json!({
                    "method": note.method,
                    "tsMs": now,
                    "params": note.params,
                }))?,
            )?;
        }
        "item/started" | "item/completed" => {
            if let Some(item) = note.params.get("item")
                && item.get("type").and_then(Value::as_str) == Some("collabAgentToolCall")
                && let Some(id) = item.get("id").and_then(Value::as_str)
            {
                doc.get_map("codex:workGraph")
                    .insert(id, serde_json::to_string(item)?)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn value_key(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

async fn handle(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    buffers: &mut Buffers,
    note: &Notification,
) -> Result<()> {
    match note.method.as_str() {
        "item/started" => handle_item_started(db, broadcast, buffers, &note.params).await,
        "item/completed" => handle_item_completed(db, broadcast, buffers, &note.params).await,
        "item/agentMessage/delta" | "item/reasoning/textDelta" => {
            handle_text_delta(db, broadcast, buffers, &note.params, TextField::Text).await
        }
        "item/reasoning/summaryTextDelta" => {
            handle_text_delta(db, broadcast, buffers, &note.params, TextField::Summary).await
        }
        "item/commandExecution/outputDelta" => {
            handle_command_output_delta(db, broadcast, buffers, &note.params).await
        }
        // Failures and interruptions both arrive as `turn/completed` with the
        // disposition on `turn.status`; codex has no `turn/failed` or
        // `turn/cancelled` notification.
        "turn/completed" => handle_turn_completed(db, broadcast, buffers, &note.params).await,
        "turn/plan/updated" => handle_plan_updated(db, broadcast, &note.params).await,
        "thread/name/updated" => handle_thread_name_updated(db, broadcast, &note.params).await,
        "thread/goal/updated" => handle_goal_updated(db, broadcast, &note.params).await,
        "thread/goal/cleared" => handle_goal_cleared(db, broadcast, &note.params).await,
        // Mid-turn / account errors (transient retries, auth) also arrive here.
        // Terminal failures are recorded by the `turn/completed` path above, so
        // this only logs (recording again would duplicate the system row, and a
        // transient `willRetry` error must not flip the thread to errored).
        "error" => {
            log_error_notification(&note.params);
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn handle_plan_updated(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    params: &Value,
) -> Result<()> {
    let thread_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .context("turn/plan/updated missing threadId")?;
    let explanation = params
        .get("explanation")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let steps: Vec<PlanStep> = match params.get("plan") {
        Some(v) => {
            serde_json::from_value(v.clone()).context("turn/plan/updated has invalid plan")?
        }
        None => Vec::new(),
    };
    let plan = ThreadPlan { explanation, steps };

    let updated = {
        let db = db.lock().await;
        db.set_thread_plan(thread_id, Some(&plan), now_ms())?
    };
    if let Some(thread) = updated {
        let _ = broadcast.send(Delta::ThreadUpsert { thread });
    }
    Ok(())
}

// `thread/goal/updated` carries a `{ threadId, goal }` payload where
// `goal` is the materialized goal record (objective, status, token /
// time budgets). Each emission is authoritative; we replace the
// stored goal in full and broadcast the thread row.
async fn handle_goal_updated(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    params: &Value,
) -> Result<()> {
    let thread_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .context("thread/goal/updated missing threadId")?;
    let goal_value = params
        .get("goal")
        .context("thread/goal/updated missing goal")?;
    let goal: ThreadGoal = serde_json::from_value(goal_value.clone())
        .context("thread/goal/updated has invalid goal")?;

    let updated = {
        let db = db.lock().await;
        db.set_thread_goal(thread_id, Some(&goal), now_ms())?
    };
    if let Some(thread) = updated {
        let _ = broadcast.send(Delta::ThreadUpsert { thread });
    }
    Ok(())
}

async fn handle_goal_cleared(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    params: &Value,
) -> Result<()> {
    let thread_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .context("thread/goal/cleared missing threadId")?;
    let updated = {
        let db = db.lock().await;
        db.set_thread_goal(thread_id, None, now_ms())?
    };
    if let Some(thread) = updated {
        let _ = broadcast.send(Delta::ThreadUpsert { thread });
    }
    Ok(())
}

// ---------------------------------------------------------------------
// item/started, deltas, item/completed
// ---------------------------------------------------------------------

async fn handle_item_started(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    buffers: &mut Buffers,
    params: &Value,
) -> Result<()> {
    let Some((thread_id, item, item_id)) = item_target(params) else {
        return Ok(());
    };
    let kind = ItemKind::parse(item.get("type").and_then(Value::as_str).unwrap_or(""));
    if matches!(kind, ItemKind::Unknown) {
        return Ok(());
    }
    let started_at_ms = params
        .get("startedAtMs")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_ms);

    let mut buf = ItemBuffer {
        thread_id: thread_id.to_owned(),
        kind,
        started_at_ms,
        ..Default::default()
    };
    populate_from_item(&mut buf, item);
    flush(db, broadcast, &item_id, &buf, BumpThread::Yes).await?;
    buffers.insert(item_id, buf);
    Ok(())
}

#[derive(Clone, Copy)]
enum TextField {
    Text,
    Summary,
}

async fn handle_text_delta(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    buffers: &mut Buffers,
    params: &Value,
    field: TextField,
) -> Result<()> {
    let Some((item_id, delta)) = delta_target(params) else {
        return Ok(());
    };
    let Some(buf) = buffers.get_mut(&item_id) else {
        return Ok(()); // delta without started — rare; skip rather than guess kind
    };
    match field {
        TextField::Text => buf.text.push_str(delta),
        TextField::Summary => buf.summary.push_str(delta),
    }
    flush(db, broadcast, &item_id, buf, BumpThread::No).await
}

async fn handle_command_output_delta(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    buffers: &mut Buffers,
    params: &Value,
) -> Result<()> {
    let Some((item_id, delta)) = delta_target(params) else {
        return Ok(());
    };
    let Some(buf) = buffers.get_mut(&item_id) else {
        return Ok(());
    };
    buf.command_output.push_str(delta);
    flush(db, broadcast, &item_id, buf, BumpThread::No).await
}

async fn handle_item_completed(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    buffers: &mut Buffers,
    params: &Value,
) -> Result<()> {
    let Some((thread_id, item, item_id)) = item_target(params) else {
        return Ok(());
    };
    let completed_at_ms = params
        .get("completedAtMs")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_ms);
    let kind = ItemKind::parse(item.get("type").and_then(Value::as_str).unwrap_or(""));

    // Reuse the streaming buffer when we have one (so accumulated
    // command_output etc. survives) and synthesize one when the
    // started/delta sequence was skipped (item arrives terminal-only).
    let mut buf = buffers.remove(&item_id).unwrap_or_else(|| ItemBuffer {
        thread_id: thread_id.to_owned(),
        started_at_ms: completed_at_ms,
        ..Default::default()
    });
    buf.kind = kind;
    buf.completed_at_ms = Some(completed_at_ms);
    populate_from_item(&mut buf, item);
    flush(db, broadcast, &item_id, &buf, BumpThread::Yes).await
}

// ---------------------------------------------------------------------
// turn/completed, thread/name/updated
// ---------------------------------------------------------------------

async fn handle_turn_completed(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    buffers: &mut Buffers,
    params: &Value,
) -> Result<()> {
    // A failed run still arrives as `turn/completed`, with the cause
    // carried in `turn.status == "failed"` / `turn.error.message`
    // (e.g. a `usageLimitExceeded` rejection). Treating it like a
    // success flips the thread back to idle with no assistant reply and
    // no explanation, so the user just sees their prompt and silence.
    // Surface it as a system row and mark the thread errored instead.
    if let Some(turn) = params.get("turn")
        && turn.get("status").and_then(Value::as_str) == Some("failed")
        && let Some(thread_id) = params
            .get("threadId")
            .or_else(|| turn.get("threadId"))
            .and_then(Value::as_str)
    {
        let reason = turn
            .get("error")
            .and_then(|err| err.get("message"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("the model run failed");
        buffers.retain(|_, buf| buf.thread_id != thread_id);
        return record_system_error(db, broadcast, thread_id, reason).await;
    }
    // An interrupted turn (codex's terminal status for a cancelled run; accept
    // "cancelled" too, defensively) should land "cancelled", not "idle", so the
    // UI distinguishes a user stop from a clean finish.
    if let Some(turn) = params.get("turn")
        && matches!(
            turn.get("status").and_then(Value::as_str),
            Some("interrupted" | "cancelled")
        )
    {
        return handle_turn_terminal(db, broadcast, buffers, params, "cancelled").await;
    }
    handle_turn_terminal(db, broadcast, buffers, params, "idle").await
}

/// Log a codex `error` notification. The user-visible row for a terminal
/// failure is written by `handle_turn_completed`; this gives operators
/// visibility into the error kind and into transient (`willRetry`) errors that
/// never reach a `turn/completed`.
fn log_error_notification(params: &Value) {
    let error = params.get("error");
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("unknown codex error");
    let retrying = params
        .get("willRetry")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match error_info(error).filter(|info| !info.is_empty()) {
        Some(info) => eprintln!("room: codex error ({info}, willRetry={retrying}): {message}"),
        None => eprintln!("room: codex error (willRetry={retrying}): {message}"),
    }
}

/// `codexErrorInfo` is a string-or-object enum: bare tags like
/// `"usageLimitExceeded"` serialize as strings, variants with payload
/// (`{ "httpConnectionFailed": { .. } }`) as a single-key object. Return the
/// tag in either form so HTTP/connection failures keep their kind in the log.
fn error_info(error: Option<&Value>) -> Option<String> {
    let info = error?.get("codexErrorInfo")?;
    info.as_str()
        .map(str::to_owned)
        .or_else(|| info.as_object().and_then(|o| o.keys().next().cloned()))
}

async fn handle_turn_terminal(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    buffers: &mut Buffers,
    params: &Value,
    status: &str,
) -> Result<()> {
    let Some(thread_id) = params.get("threadId").and_then(Value::as_str) else {
        return Ok(());
    };
    // Stale buffers can linger if a turn aborts before the matching
    // item/completed; clear them now so the next turn starts clean.
    buffers.retain(|_, buf| buf.thread_id != thread_id);
    let updated = {
        let db = db.lock().await;
        db.set_thread_status(thread_id, status, now_ms())?
    };
    if let Some(thread) = updated {
        let _ = broadcast.send(Delta::ThreadUpsert { thread });
    }
    Ok(())
}

async fn handle_thread_name_updated(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    params: &Value,
) -> Result<()> {
    let Some(thread_id) = params.get("threadId").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Ok(());
    };
    if name.is_empty() {
        return Ok(());
    }
    let updated = {
        let db = db.lock().await;
        db.set_thread_title_if_default(thread_id, &derive_title(name))?;
        db.get_thread(thread_id)?
    };
    if let Some(thread) = updated {
        let _ = broadcast.send(Delta::ThreadUpsert { thread });
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Buffer ↔ wire
// ---------------------------------------------------------------------

/// Overlay the canonical fields from a wire `item` onto a buffer. The
/// same routine is used for `item/started` (where most fields are
/// absent) and `item/completed` (where every field is authoritative),
/// because every assignment is no-op for missing fields. Streaming
/// accumulators (`text`, `summary`, `command_output`) are only
/// overwritten when the wire carries a final value — partial item
/// payloads can't replace what the deltas already accumulated.
fn populate_from_item(buf: &mut ItemBuffer, item: &Value) {
    match buf.kind {
        ItemKind::AgentMessage => {
            if let Some(text) = item.get("text").and_then(Value::as_str)
                && !text.is_empty()
            {
                buf.text = text.to_owned();
            }
        }
        ItemKind::Reasoning => {
            let content = join_strings(item.get("content"));
            if !content.is_empty() {
                buf.text = content;
            }
            let summary = join_strings(item.get("summary"));
            if !summary.is_empty() {
                buf.summary = summary;
            }
        }
        ItemKind::CommandExecution => {
            buf.tool_name = Some("shell".to_owned());
            buf.tool_input = Some(json!({
                "command": item.get("command"),
                "cwd": item.get("cwd"),
            }));
            if let Some(out) = item.get("aggregatedOutput").and_then(Value::as_str)
                && !out.is_empty()
            {
                buf.command_output = out.to_owned();
            }
            // Map codex `status` + `exitCode` onto a typed result; stays
            // `None` (-> Running) while in progress.
            if let Some(result) = ToolResult::from_command(item, &buf.command_output) {
                buf.result = Some(result);
            }
        }
        ItemKind::FileChange => {
            buf.tool_name = Some("apply_patch".to_owned());
            if let Some(changes) = item.get("changes") {
                buf.tool_input = Some(changes.clone());
                // No unified diff on the wire; serialise the
                // FileUpdateChange list and let the UI's diff renderer
                // interpret it.
                buf.patch = Some(serde_json::to_string(changes).unwrap_or_default());
            }
            // Records apply success/failure; the diff itself rides `patch`.
            if let Some(result) = ToolResult::from_file_change(item) {
                buf.result = Some(result);
            }
        }
        ItemKind::McpToolCall => {
            buf.tool_name = Some(format!(
                "{}::{}",
                item.get("server").and_then(Value::as_str).unwrap_or("mcp"),
                item.get("tool").and_then(Value::as_str).unwrap_or("?"),
            ));
            if let Some(args) = item.get("arguments") {
                buf.tool_input = Some(args.clone());
            }
            // Reads `status`, `result`, and `error` together: a failed
            // call surfaces its `error.message` instead of the old
            // stringified `null`.
            if let Some(result) = ToolResult::from_mcp(item) {
                buf.result = Some(result);
            }
        }
        ItemKind::WebSearch => {
            buf.tool_name = Some("web_search".to_owned());
            if let Some(query) = item.get("query") {
                buf.tool_input = Some(query.clone());
            }
            if let Some(result) = ToolResult::from_web_search(item) {
                buf.result = Some(result);
            }
        }
        ItemKind::Unknown => {}
    }
}

/// Build the Message row for a buffer at its current state. Returns
/// None for kinds we don't render.
fn render_message(item_id: &str, buf: &ItemBuffer) -> Option<Message> {
    let id = synth_message_id(item_id);
    let ts_ms = buf.ts();
    let base = |role: &str, kind: &str| Message {
        id: id.clone(),
        thread_id: buf.thread_id.clone(),
        ts_ms,
        role: role.to_owned(),
        kind: kind.to_owned(),
        text: None,
        tool_name: None,
        tool_use_id: None,
        tool_input: None,
        result: None,
        patch: None,
        images: Vec::new(),
    };
    let nonempty = |s: &str| (!s.is_empty()).then(|| s.to_owned());
    let tool_use_id = (!item_id.is_empty()).then(|| item_id.to_owned());

    match buf.kind {
        ItemKind::AgentMessage => Some(Message {
            text: Some(buf.text.clone()),
            ..base("assistant", "assistant_text")
        }),
        ItemKind::Reasoning => {
            // Codex emits a reasoning shell on every turn, but gpt-5
            // returns an empty summary for trivial turns even with
            // `summary: detailed`. Skip the empty ones so the
            // transcript doesn't show a hollow row per turn.
            let text = if !buf.text.is_empty() {
                &buf.text
            } else {
                &buf.summary
            };
            if text.is_empty() {
                None
            } else {
                Some(Message {
                    text: Some(text.to_owned()),
                    ..base("assistant", "thinking")
                })
            }
        }
        ItemKind::CommandExecution => Some(Message {
            tool_name: buf.tool_name.clone(),
            tool_use_id,
            tool_input: buf.tool_input.clone(),
            // While the command streams (no terminal status yet) show the
            // partial stdout/stderr as a Running result.
            result: Some(
                buf.result
                    .clone()
                    .unwrap_or_else(|| ToolResult::running(nonempty(&buf.command_output))),
            ),
            ..base("tool", "tool_call")
        }),
        ItemKind::FileChange | ItemKind::McpToolCall | ItemKind::WebSearch => Some(Message {
            tool_name: buf.tool_name.clone(),
            tool_use_id,
            tool_input: buf.tool_input.clone(),
            result: Some(
                buf.result
                    .clone()
                    .unwrap_or(ToolResult::Running { display: None }),
            ),
            patch: buf.patch.clone(),
            ..base("tool", "tool_call")
        }),
        ItemKind::Unknown => None,
    }
}

/// Whether `flush` should also re-read the thread row and broadcast a
/// `ThreadUpsert`. Pure delta writes skip this (the started/completed
/// brackets handle the thread-row bump for the whole item).
#[derive(Clone, Copy, PartialEq, Eq)]
enum BumpThread {
    Yes,
    No,
}

async fn flush(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    item_id: &str,
    buf: &ItemBuffer,
    bump_thread: BumpThread,
) -> Result<()> {
    let Some(message) = render_message(item_id, buf) else {
        return Ok(());
    };
    let thread_id = buf.thread_id.clone();
    let (msg, was_new, thread) = {
        let db = db.lock().await;
        // A turn opened through the `Engine` trait (`/api/agent/turns`)
        // never ran the chat path's `record_user_prompt`, so its thread
        // row may not exist yet when the first item streams in. Seed it
        // here so the message FK holds; a chat-path thread already
        // exists and this is a no-op.
        db.ensure_thread(&thread_id, buf.ts())?;
        let (msg, was_new) = db.upsert_message(&message)?;
        let thread = if bump_thread == BumpThread::Yes {
            db.get_thread(&thread_id)?
        } else {
            None
        };
        (msg, was_new, thread)
    };
    let delta = if was_new {
        Delta::MessageAppend {
            thread_id: thread_id.clone(),
            message: msg,
        }
    } else {
        Delta::MessageUpdate {
            thread_id: thread_id.clone(),
            message: msg,
        }
    };
    let _ = broadcast.send(delta);
    if let Some(thread) = thread {
        let _ = broadcast.send(Delta::ThreadUpsert { thread });
    }
    Ok(())
}

// ---------------------------------------------------------------------
// HTTP entry points
// ---------------------------------------------------------------------

/// Run a complete user turn against codex: ensure a thread exists,
/// record the user message, dispatch `turn/start`, and let the bridge
/// stream the resulting items back through the broadcast.
///
/// Returns the room thread id that received the turn. If `thread_id`
/// is supplied it's used as-is (and must already correspond to a real
/// codex thread); otherwise a fresh codex thread is created and the
/// new id is returned.
pub async fn submit_user_turn(
    client: &CodexClient,
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    thread_id: Option<String>,
    text: &str,
    images: &[String],
    input_items: &[Value],
    cwd: Option<&str>,
    author: &str,
    options: &TurnOptions,
) -> Result<String> {
    submit_turn(
        client,
        db,
        broadcast,
        thread_id,
        text,
        images,
        input_items,
        cwd,
        author,
        options,
    )
    .await
}

pub async fn submit_workflow_turn(
    client: &CodexClient,
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    thread_id: Option<String>,
    text: &str,
    cwd: Option<&str>,
    author: &str,
    options: TurnOptions,
) -> Result<String> {
    submit_turn(
        client,
        db,
        broadcast,
        thread_id,
        text,
        &[],
        &[],
        cwd,
        author,
        &options,
    )
    .await
}

async fn submit_turn(
    client: &CodexClient,
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    thread_id: Option<String>,
    text: &str,
    images: &[String],
    input_items: &[Value],
    cwd: Option<&str>,
    author: &str,
    options: &TurnOptions,
) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() && images.is_empty() && input_items.is_empty() {
        anyhow::bail!("input is empty");
    }
    let now = now_ms();

    let resolved_id = match thread_id.filter(|s| !s.is_empty()) {
        Some(id) => id,
        None => start_codex_thread(client, cwd, options).await?,
    };

    record_user_prompt(
        db,
        broadcast,
        &resolved_id,
        trimmed,
        images,
        cwd,
        author,
        now,
        options,
    )
    .await?;

    // The bridge handles `item/*` notifications out-of-band, so we
    // don't need to wait for `turn/completed` — the HTTP client gets a
    // fast ACK back and the UI stays responsive while items stream in
    // through the WS channel. We do await the turn/start response so
    // schema or auth errors surface as a 5xx instead of disappearing
    // into the bridge logs.
    //
    // Codex's v2 `UserInput` enum (codex-rs/app-server-protocol/src/
    // protocol/v2.rs) accepts `{type: "text", text}` and
    // `{type: "image", url}` (the latter taking a data URL or http URL).
    // The model sees text+images in the order they appear here.
    let input_items = if input_items.is_empty() {
        let mut built: Vec<Value> = Vec::new();
        if !trimmed.is_empty() {
            built.push(json!({"type": "text", "text": trimmed}));
        }
        for url in images {
            built.push(json!({"type": "image", "url": url}));
        }
        built
    } else {
        input_items.to_vec()
    };
    let mut turn_params = serde_json::Map::new();
    turn_params.insert("threadId".to_owned(), Value::String(resolved_id.clone()));
    turn_params.insert("input".to_owned(), Value::Array(input_items));
    if let Some(cwd) = cwd.filter(|s| !s.is_empty()) {
        turn_params.insert("cwd".to_owned(), Value::String(cwd.to_owned()));
    }
    if let Some(model) = options.model.as_ref().filter(|s| !s.is_empty()) {
        turn_params.insert("model".to_owned(), Value::String(model.to_owned()));
    }
    if let Some(policy) = options.approval_policy.clone() {
        turn_params.insert("approvalPolicy".to_owned(), policy);
    }
    if let Some(profile) = options
        .permission_profile
        .as_ref()
        .filter(|s| !s.is_empty())
    {
        turn_params.insert("permissions".to_owned(), Value::String(profile.to_owned()));
    } else if let Some(sandbox) = options.sandbox.as_deref().filter(|s| !s.is_empty()) {
        turn_params.insert("sandboxPolicy".to_owned(), sandbox_policy(sandbox));
    } else {
        turn_params.insert(
            "sandboxPolicy".to_owned(),
            json!({"type": "dangerFullAccess"}),
        );
    }
    // Mirrors the codex app-server `ReasoningEffort` enum
    // (none/minimal/low/medium/high/xhigh). Hardcoded to the max so
    // room chats get the same depth as a `xhigh` skill.
    turn_params.insert(
        "effort".to_owned(),
        Value::String(options.effort.clone().unwrap_or_else(|| "xhigh".to_owned())),
    );
    turn_params.insert(
        "summary".to_owned(),
        Value::String(
            options
                .summary
                .clone()
                .unwrap_or_else(|| "detailed".to_owned()),
        ),
    );
    put_optional_string(&mut turn_params, "title", options.title.as_deref());

    if let Err(err) = client
        .request("turn/start", Value::Object(turn_params))
        .await
    {
        // The user already saw their prompt land; without a marker the
        // thread would sit "active" forever and they'd never know why
        // there was no reply. Surface the failure as a system row and
        // flip the thread to errored.
        let _ = record_system_error(db, broadcast, &resolved_id, &format!("{err:#}")).await;
        return Err(err.context("codex turn/start"));
    }

    Ok(resolved_id)
}

/// Set the goal on a codex thread. Codex echoes the new state back
/// through a `thread/goal/updated` notification, which the bridge
/// persists and broadcasts; this just dispatches the RPC and lets
/// the normal notification path do the rest.
pub async fn submit_goal_set(
    client: &CodexClient,
    thread_id: &str,
    objective: &str,
    token_budget: Option<i64>,
) -> Result<()> {
    let objective = objective.trim();
    if objective.is_empty() {
        anyhow::bail!("goal objective is empty");
    }
    let mut params = serde_json::Map::new();
    params.insert("threadId".to_owned(), Value::String(thread_id.to_owned()));
    params.insert("objective".to_owned(), Value::String(objective.to_owned()));
    if let Some(budget) = token_budget {
        params.insert("tokenBudget".to_owned(), json!(budget));
    }
    client
        .request("thread/goal/set", Value::Object(params))
        .await
        .context("codex thread/goal/set")?;
    Ok(())
}

/// Clear the goal on a codex thread. Codex echoes the clear through
/// `thread/goal/cleared`; see `submit_goal_set` for the symmetry.
pub async fn submit_goal_clear(client: &CodexClient, thread_id: &str) -> Result<()> {
    let params = json!({ "threadId": thread_id });
    client
        .request("thread/goal/clear", params)
        .await
        .context("codex thread/goal/clear")?;
    Ok(())
}

async fn start_codex_thread(
    client: &CodexClient,
    cwd: Option<&str>,
    options: &TurnOptions,
) -> Result<String> {
    let mut params = serde_json::Map::new();
    if let Some(cwd) = cwd.filter(|s| !s.is_empty()) {
        params.insert("cwd".to_owned(), Value::String(cwd.to_owned()));
    }
    if let Some(model) = options.model.as_ref().filter(|s| !s.is_empty()) {
        params.insert("model".to_owned(), Value::String(model.to_owned()));
    }
    if let Some(policy) = options.approval_policy.clone() {
        params.insert("approvalPolicy".to_owned(), policy);
    }
    if let Some(profile) = options
        .permission_profile
        .as_ref()
        .filter(|s| !s.is_empty())
    {
        params.insert("permissions".to_owned(), Value::String(profile.to_owned()));
    } else {
        params.insert(
            "sandbox".to_owned(),
            Value::String(
                options
                    .sandbox
                    .clone()
                    .unwrap_or_else(|| "danger-full-access".to_owned()),
            ),
        );
    }
    if !options.dynamic_tools.is_empty() {
        params.insert(
            "dynamicTools".to_owned(),
            Value::Array(options.dynamic_tools.clone()),
        );
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

fn put_optional_string(params: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|s| !s.is_empty()) {
        params.insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

fn sandbox_policy(value: &str) -> Value {
    match value {
        "workspace-write" => json!({ "type": "workspaceWrite", "networkAccess": true }),
        "read-only" => json!({ "type": "readOnly", "networkAccess": true }),
        "danger-full-access" => json!({ "type": "dangerFullAccess" }),
        other => json!({ "type": other }),
    }
}

async fn record_user_prompt(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    thread_id: &str,
    text: &str,
    images: &[String],
    cwd: Option<&str>,
    author: &str,
    now: i64,
    options: &TurnOptions,
) -> Result<()> {
    // An image-only prompt would derive an empty title/preview; fall
    // back to a placeholder so the sidebar row still reads.
    let placeholder = "(image attachment)";
    let preview_source = if text.is_empty() { placeholder } else { text };
    let workspace = workspace::snapshot(cwd).await.ok();
    let message = Message {
        id: Ulid::new().to_string(),
        thread_id: thread_id.to_owned(),
        ts_ms: now,
        role: "user".to_owned(),
        kind: "user_prompt".to_owned(),
        text: Some(text.to_owned()),
        tool_name: Some(format!("chat:{author}")),
        tool_use_id: None,
        tool_input: None,
        result: None,
        patch: None,
        images: images.to_vec(),
    };
    let (inserted, thread) = {
        let db = db.lock().await;
        db.upsert_thread(&ThreadUpsert {
            id: thread_id.to_owned(),
            user: author.to_owned(),
            host: String::new(),
            repo: workspace.as_ref().and_then(|w| w.repo.clone()),
            branch: workspace.as_ref().and_then(|w| w.branch.clone()),
            cwd: workspace
                .as_ref()
                .map(|w| w.cwd.clone())
                .or_else(|| cwd.map(str::to_owned)),
            workspace_root: workspace.as_ref().map(|w| w.root.clone()),
            base_sha: workspace.as_ref().and_then(|w| w.base_sha.clone()),
            model: options.model.clone(),
            reasoning_effort: options.effort.clone(),
            approval_policy: options.approval_policy.clone(),
            permission_profile: options.permission_profile.clone(),
            title_if_empty: Some(derive_title(preview_source)),
            status: Some("active".to_owned()),
            now_ms: now,
            preview: Some(derive_preview(preview_source)),
        })?;
        let inserted = db.insert_message(&message)?;
        let thread = db.get_thread(thread_id)?;
        (inserted, thread)
    };
    let _ = broadcast.send(Delta::MessageAppend {
        thread_id: thread_id.to_owned(),
        message: inserted,
    });
    if let Some(thread) = thread {
        let _ = broadcast.send(Delta::ThreadUpsert { thread });
    }
    Ok(())
}

async fn record_system_error(
    db: &Arc<Mutex<Db>>,
    broadcast: &broadcast::Sender<Delta>,
    thread_id: &str,
    reason: &str,
) -> Result<()> {
    let now = now_ms();
    let message = Message {
        id: Ulid::new().to_string(),
        thread_id: thread_id.to_owned(),
        ts_ms: now,
        role: "system".to_owned(),
        // Distinct from a benign system notice so the UI renders it as a
        // contained, readable error block rather than the centered divider.
        kind: "error".to_owned(),
        text: Some(format!("turn failed: {reason}")),
        tool_name: None,
        tool_use_id: None,
        tool_input: None,
        result: None,
        patch: None,
        images: Vec::new(),
    };
    let (inserted, thread) = {
        let db = db.lock().await;
        let inserted = db.insert_message(&message)?;
        let thread = db.set_thread_status(thread_id, "errored", now)?;
        (inserted, thread)
    };
    let _ = broadcast.send(Delta::MessageAppend {
        thread_id: thread_id.to_owned(),
        message: inserted,
    });
    if let Some(thread) = thread {
        let _ = broadcast.send(Delta::ThreadUpsert { thread });
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Wire helpers
// ---------------------------------------------------------------------

/// Common destructure for `item/started` and `item/completed` params.
/// Returns `(threadId, item, itemId)` or None when any required field
/// is missing — caller treats that as "skip this notification".
fn item_target(params: &Value) -> Option<(&str, &Value, String)> {
    let thread_id = params.get("threadId").and_then(Value::as_str)?;
    let item = params.get("item")?;
    let item_id = item
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    Some((thread_id, item, item_id.to_owned()))
}

/// Common destructure for the four delta notifications. Returns
/// `(itemId, delta)` or None when either field is missing/empty.
fn delta_target(params: &Value) -> Option<(String, &str)> {
    let item_id = params
        .get("itemId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    let delta = params
        .get("delta")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    Some((item_id.to_owned(), delta))
}

/// Join a `Value::Array<String>` into a single newline-separated
/// string. Returns "" when the input is missing, null, or not an
/// array of strings.
fn join_strings(value: Option<&Value>) -> String {
    let Some(Value::Array(parts)) = value else {
        return String::new();
    };
    parts
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

fn synth_message_id(item_id: &str) -> String {
    if item_id.is_empty() {
        Ulid::new().to_string()
    } else {
        // Reuse the item id directly so streaming deltas and the final
        // item/completed write replace the same row instead of
        // duplicating.
        item_id.to_owned()
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ThreadUpsert;

    fn open_db() -> Db {
        let tmp = tempfile::Builder::new()
            .prefix("room-bridge-test")
            .tempdir()
            .expect("tempdir");
        let path = tmp.path().join("room.db");
        // Leak the TempDir handle: dropping it would delete the backing
        // directory out from under the still-open SQLite file.
        std::mem::forget(tmp);
        let db = Db::open(&path).expect("open db");
        db.migrate().expect("migrate");
        db
    }

    fn seed_thread(db: &Db, id: &str) {
        db.upsert_thread(&ThreadUpsert {
            id: id.to_owned(),
            user: "alice".to_owned(),
            host: "alice-dev".to_owned(),
            repo: None,
            branch: None,
            cwd: None,
            workspace_root: None,
            base_sha: None,
            model: None,
            reasoning_effort: None,
            approval_policy: None,
            permission_profile: None,
            title_if_empty: Some("t".to_owned()),
            status: Some("active".to_owned()),
            now_ms: 1_000,
            preview: None,
        })
        .expect("seed thread");
    }

    // A failed run arrives as turn/completed with status "failed"; the
    // bridge must surface the cause as a system row and mark the thread
    // errored instead of silently flipping it back to idle.
    #[tokio::test]
    async fn failed_turn_completed_surfaces_system_error() {
        let db = Arc::new(Mutex::new(open_db()));
        seed_thread(&*db.lock().await, "thread-1");
        let (tx, _rx) = broadcast::channel(16);
        let mut buffers: Buffers = HashMap::new();

        let note = Notification {
            method: "turn/completed".to_owned(),
            params: json!({
                "threadId": "thread-1",
                "turn": {
                    "id": "turn-1",
                    "status": "failed",
                    "error": {
                        "message": "You've hit your usage limit.",
                        "codexErrorInfo": "usageLimitExceeded"
                    }
                }
            }),
        };
        handle(&db, &tx, &mut buffers, &note).await.expect("handle");

        let db = db.lock().await;
        let messages = db.list_messages("thread-1", 16).expect("messages");
        let system = messages
            .iter()
            .find(|m| m.role == "system")
            .expect("a system error row");
        assert!(
            system
                .text
                .as_deref()
                .is_some_and(|t| t.contains("usage limit")),
            "system row should carry the failure reason, got {:?}",
            system.text
        );
        let thread = db.get_thread("thread-1").expect("thread").expect("exists");
        assert_eq!(thread.status, "errored");
    }

    // A normal turn/completed leaves no system row and returns the
    // thread to idle.
    #[tokio::test]
    async fn successful_turn_completed_goes_idle() {
        let db = Arc::new(Mutex::new(open_db()));
        seed_thread(&*db.lock().await, "thread-2");
        let (tx, _rx) = broadcast::channel(16);
        let mut buffers: Buffers = HashMap::new();

        let note = Notification {
            method: "turn/completed".to_owned(),
            params: json!({
                "threadId": "thread-2",
                "turn": { "id": "turn-2", "status": "completed" }
            }),
        };
        handle(&db, &tx, &mut buffers, &note).await.expect("handle");

        let db = db.lock().await;
        assert!(
            db.list_messages("thread-2", 16)
                .expect("messages")
                .iter()
                .all(|m| m.role != "system"),
            "successful turn must not create a system row"
        );
        let thread = db.get_thread("thread-2").expect("thread").expect("exists");
        assert_eq!(thread.status, "idle");
    }

    // An interrupted turn arrives as turn/completed with status
    // "interrupted"; the thread must land "cancelled", not "idle", and
    // without a system error row.
    #[tokio::test]
    async fn interrupted_turn_completed_marks_cancelled() {
        let db = Arc::new(Mutex::new(open_db()));
        seed_thread(&*db.lock().await, "thread-3");
        let (tx, _rx) = broadcast::channel(16);
        let mut buffers: Buffers = HashMap::new();

        let note = Notification {
            method: "turn/completed".to_owned(),
            params: json!({
                "threadId": "thread-3",
                "turn": { "id": "turn-3", "status": "interrupted" }
            }),
        };
        handle(&db, &tx, &mut buffers, &note).await.expect("handle");

        let db = db.lock().await;
        assert!(
            db.list_messages("thread-3", 16)
                .expect("messages")
                .iter()
                .all(|m| m.role != "system"),
            "an interrupted turn must not create a system error row"
        );
        let thread = db.get_thread("thread-3").expect("thread").expect("exists");
        assert_eq!(thread.status, "cancelled");
    }
}
