// JSON-RPC 2.0 client for `codex app-server`.
//
// We spawn the upstream `codex` binary in `app-server` mode and speak
// newline-delimited JSON-RPC over its stdio. The `"jsonrpc":"2.0"`
// envelope is omitted on the wire, matching what the app-server
// produces and what its other in-tree clients (codex-codes, the VS
// Code extension) expect.
//
// One `CodexClient` owns one child process. A reader task demuxes
// frames into three buckets:
//
//   - responses keyed by request id, delivered to the caller's
//     oneshot
//   - notifications (no id), fanned out on a broadcast channel
//   - server-initiated requests (id + method), retained until an HTTP
//     client or Room UI replies

use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicI64, Ordering},
    },
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{broadcast, mpsc, oneshot, watch},
};

/// A notification frame (no id) emitted by the app-server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// Public RPC error type.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("codex rpc error {code}: {message}")]
    Server {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error("codex rpc transport closed")]
    Closed,
}

/// One running `codex app-server` subprocess and the channels needed
/// to talk to it.
pub struct CodexClient {
    cmd_tx: mpsc::UnboundedSender<Outgoing>,
    notifications: broadcast::Sender<Notification>,
    next_id: AtomicI64,
    pending: PendingMap,
    active_turns: ActiveTurnMap,
    /// Server-initiated JSON-RPC requests currently waiting on a
    /// caller. The app-server uses these for approvals, dynamic tools,
    /// request_user_input, MCP elicitations, auth refreshes, and
    /// attestation. Room records them into Loro and only replies when a
    /// user/client decision arrives.
    server_requests: Arc<StdMutex<HashMap<i64, PendingServerRequest>>>,
    /// Flipped to false by `child_supervisor` once the subprocess
    /// exits. Watched by the manager loop in main.rs so it can
    /// respawn without polling.
    alive: watch::Receiver<bool>,
}

enum Outgoing {
    /// Regular JSON-RPC request. The reply channel is already
    /// registered in `pending` by the caller before this is enqueued.
    Request {
        id: i64,
        method: String,
        params: Value,
    },
    /// Client-initiated notification (no id, no reply).
    Notification { method: String, params: Value },
    /// Reply to a server-initiated request.
    Reply { id: i64, result: Value },
}

type PendingMap = Arc<StdMutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>>;
type ActiveTurnMap = Arc<StdMutex<HashMap<String, String>>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingServerRequest {
    pub request_id: i64,
    pub method: String,
    pub params: Value,
}

impl CodexClient {
    /// Spawn `codex app-server` and complete the `initialize`
    /// handshake. The binary path is taken from `ROOM_CODEX_BIN` if
    /// set, falling back to `codex` on `PATH` (the wrapper from
    /// `flake.nix` always sets the env var).
    pub async fn spawn() -> Result<Arc<Self>> {
        let bin = std::env::var("ROOM_CODEX_BIN").unwrap_or_else(|_| "codex".to_owned());

        let mut child: Child = Command::new(&bin)
            .arg("app-server")
            .arg("--listen")
            .arg("stdio://")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn `{bin} app-server`"))?;

        let stdin = child.stdin.take().context("codex stdin was not piped")?;
        let stdout = child.stdout.take().context("codex stdout was not piped")?;

        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Outgoing>();
        let (note_tx, _) = broadcast::channel::<Notification>(1024);
        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
        let active_turns: ActiveTurnMap = Arc::new(StdMutex::new(HashMap::new()));
        let server_requests = Arc::new(StdMutex::new(HashMap::new()));
        let (alive_tx, alive_rx) = watch::channel(true);

        tokio::spawn(reader_task(
            stdout,
            pending.clone(),
            active_turns.clone(),
            server_requests.clone(),
            note_tx.clone(),
        ));
        tokio::spawn(writer_task(stdin, cmd_rx));
        tokio::spawn(child_supervisor(child, pending.clone(), alive_tx));

        let client = Arc::new(Self {
            cmd_tx,
            notifications: note_tx,
            next_id: AtomicI64::new(1),
            pending,
            active_turns,
            server_requests,
            alive: alive_rx,
        });

        client
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "room-server",
                        "title": "Room (multiplayer Codex viewer)",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": {
                        "experimentalApi": true
                    }
                }),
            )
            .await
            .context("codex initialize handshake")?;
        client.notify("initialized", json!({}))?;

        Ok(client)
    }

    /// Subscribe to the raw notification stream. Each subscriber gets
    /// every notification from this client; filter on the caller side
    /// by `method` / `threadId` / `turnId`.
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.notifications.subscribe()
    }

    pub fn pending_server_requests(&self) -> Vec<PendingServerRequest> {
        let mut requests: Vec<_> = self
            .server_requests
            .lock()
            .expect("server_requests mutex poisoned")
            .values()
            .cloned()
            .collect();
        requests.sort_by_key(|request| request.request_id);
        requests
    }

    /// Await the moment the underlying `codex app-server` subprocess
    /// exits. Returns immediately if it has already exited. Each call
    /// gets its own watch::Receiver clone, so multiple supervisors
    /// can wait on the same client.
    pub async fn wait_for_exit(&self) {
        let mut rx = self.alive.clone();
        while *rx.borrow() {
            if rx.changed().await.is_err() {
                // Sender dropped — subprocess is gone.
                return;
            }
        }
    }

    /// Send a JSON-RPC request and await its `result`.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = oneshot::channel();
        // Register the reply slot BEFORE pushing the request onto the
        // writer queue. Without this the reader can race and find no
        // entry to deliver the response into, since the writer flush
        // and the reader read are independent OS-scheduled tasks.
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .insert(id, reply_tx);

        if self
            .cmd_tx
            .send(Outgoing::Request {
                id,
                method: method.to_owned(),
                params,
            })
            .is_err()
        {
            // Writer task is gone — also drain the slot so we don't
            // leak it.
            self.pending
                .lock()
                .expect("pending mutex poisoned")
                .remove(&id);
            return Err(anyhow!("codex client closed"));
        }

        match reply_rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(anyhow::Error::from(e)),
            Err(_) => Err(anyhow!("codex client closed before responding")),
        }
    }

    pub fn active_turn_id(&self, thread_id: &str) -> Option<String> {
        self.active_turns
            .lock()
            .expect("active_turns mutex poisoned")
            .get(thread_id)
            .cloned()
    }

    pub async fn interrupt_active_turn(&self, thread_id: &str) -> Result<()> {
        let turn_id = self
            .active_turn_id(thread_id)
            .with_context(|| format!("thread {thread_id} has no active codex turn"))?;
        self.request(
            "turn/interrupt",
            json!({
                "threadId": thread_id,
                "turnId": turn_id,
            }),
        )
        .await
        .context("codex turn/interrupt")?;
        Ok(())
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.cmd_tx
            .send(Outgoing::Notification {
                method: method.to_owned(),
                params,
            })
            .map_err(|_| anyhow!("codex client closed"))?;
        Ok(())
    }

    /// Reply to a server-initiated app-server request. The caller passes
    /// the exact JSON result shape Codex expects for that request method.
    pub fn reply_server_request(&self, request_id: i64, result: Value) -> Result<()> {
        let existed = self
            .server_requests
            .lock()
            .expect("server_requests mutex poisoned")
            .remove(&request_id);
        if existed.is_none() {
            anyhow::bail!("unknown or already-resolved codex request id {request_id}");
        }
        self.cmd_tx
            .send(Outgoing::Reply {
                id: request_id,
                result,
            })
            .map_err(|_| anyhow!("codex client closed"))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------

async fn reader_task(
    stdout: ChildStdout,
    pending: PendingMap,
    active_turns: ActiveTurnMap,
    server_requests: Arc<StdMutex<HashMap<i64, PendingServerRequest>>>,
    notifications: broadcast::Sender<Notification>,
) {
    // The CLI rarely exceeds a few KB per message but item/* payloads
    // (especially file changes) can be large. BufReader keeps each
    // parse to one line allocation.
    let mut lines = BufReader::with_capacity(64 * 1024, stdout).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => {
                eprintln!("room: codex app-server stdout closed");
                break;
            }
            Err(err) => {
                eprintln!("room: codex stdout read error: {err}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let frame: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("room: codex emitted non-JSON line ({err}): {line}");
                continue;
            }
        };
        let id = frame.get("id").and_then(Value::as_i64);
        let method = frame
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);

        match (id, method.as_deref()) {
            // Response to one of our requests.
            (Some(rid), None) => {
                let tx = pending.lock().expect("pending mutex poisoned").remove(&rid);
                if let Some(tx) = tx {
                    if let Some(err) = frame.get("error") {
                        let code = err.get("code").and_then(Value::as_i64).unwrap_or(-32000);
                        let message = err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();
                        let data = err.get("data").cloned();
                        let _ = tx.send(Err(RpcError::Server {
                            code,
                            message,
                            data,
                        }));
                    } else {
                        let _ = tx.send(Ok(frame.get("result").cloned().unwrap_or(Value::Null)));
                    }
                } else {
                    eprintln!("room: codex response for unknown id {rid}: {line}");
                }
            }
            // Server-initiated request (approvals, dynamic tool calls,
            // request_user_input, auth refresh, attestation, MCP
            // elicitations). Do not auto-deny: the bridge records the
            // request into Loro so the desktop UI can answer it.
            (Some(rid), Some(method)) => {
                let params = frame.get("params").cloned().unwrap_or(Value::Null);
                server_requests
                    .lock()
                    .expect("server_requests mutex poisoned")
                    .insert(
                        rid,
                        PendingServerRequest {
                            request_id: rid,
                            method: method.to_owned(),
                            params: params.clone(),
                        },
                    );
                let n = Notification {
                    method: "server/request".to_owned(),
                    params: json!({
                        "requestId": rid,
                        "method": method,
                        "params": params,
                    }),
                };
                let _ = notifications.send(n);
            }
            // Notification.
            (None, Some(_)) => {
                let params = frame.get("params").cloned().unwrap_or(Value::Null);
                let n = Notification {
                    method: method.unwrap(),
                    params,
                };
                update_active_turns(&active_turns, &n.method, &n.params);
                // Ignore send errors: notifications are best-effort.
                // A missing subscriber just means nobody's listening
                // this instant; the next subscriber will see future
                // events.
                let _ = notifications.send(n);
            }
            _ => {
                eprintln!("room: codex sent an unrecognised frame: {line}");
            }
        }
    }
}

fn update_active_turns(active_turns: &ActiveTurnMap, method: &str, params: &Value) {
    match method {
        "turn/started" => {
            if let (Some(thread_id), Some(turn_id)) = (thread_id(params), turn_id(params)) {
                active_turns
                    .lock()
                    .expect("active_turns mutex poisoned")
                    .insert(thread_id.to_owned(), turn_id.to_owned());
            }
        }
        "turn/completed" => {
            if let Some(thread_id) = thread_id(params) {
                active_turns
                    .lock()
                    .expect("active_turns mutex poisoned")
                    .remove(thread_id);
            }
        }
        _ => {}
    }
}

fn thread_id(params: &Value) -> Option<&str> {
    params
        .get("threadId")
        .or_else(|| {
            params
                .get("thread")
                .and_then(|thread| thread.get("threadId"))
        })
        .or_else(|| params.get("turn").and_then(|turn| turn.get("threadId")))
        .and_then(Value::as_str)
}

fn turn_id(params: &Value) -> Option<&str> {
    params
        .get("turnId")
        .or_else(|| params.get("turn").and_then(|turn| turn.get("id")))
        .and_then(Value::as_str)
}

async fn writer_task(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<Outgoing>) {
    while let Some(msg) = rx.recv().await {
        let line = match msg {
            Outgoing::Request { id, method, params } => {
                json!({"id": id, "method": method, "params": params}).to_string()
            }
            Outgoing::Notification { method, params } => {
                json!({"method": method, "params": params}).to_string()
            }
            Outgoing::Reply { id, result } => json!({"id": id, "result": result}).to_string(),
        };
        if let Err(err) = stdin.write_all(line.as_bytes()).await {
            eprintln!("room: codex stdin write error: {err}");
            break;
        }
        if let Err(err) = stdin.write_all(b"\n").await {
            eprintln!("room: codex stdin newline error: {err}");
            break;
        }
        if let Err(err) = stdin.flush().await {
            eprintln!("room: codex stdin flush error: {err}");
            break;
        }
    }
}

async fn child_supervisor(mut child: Child, pending: PendingMap, alive_tx: watch::Sender<bool>) {
    match child.wait().await {
        Ok(status) => eprintln!("room: codex app-server exited: {status}"),
        Err(err) => eprintln!("room: codex app-server wait error: {err}"),
    }
    // Tell external supervisors first so they can start respawning
    // while we drain. The drain unblocks any in-flight requests with
    // RpcError::Closed; without it callers would hang on their
    // oneshots.
    let _ = alive_tx.send(false);
    let mut map = pending.lock().expect("pending mutex poisoned");
    for (_, tx) in map.drain() {
        let _ = tx.send(Err(RpcError::Closed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_turns() -> ActiveTurnMap {
        Arc::new(StdMutex::new(HashMap::new()))
    }

    #[test]
    fn tracks_active_turn_from_top_level_fields() {
        let turns = active_turns();

        update_active_turns(
            &turns,
            "turn/started",
            &json!({ "threadId": "thread-1", "turnId": "turn-1" }),
        );

        assert_eq!(
            turns
                .lock()
                .expect("active_turns mutex poisoned")
                .get("thread-1")
                .map(String::as_str),
            Some("turn-1")
        );

        update_active_turns(&turns, "turn/completed", &json!({ "threadId": "thread-1" }));

        assert!(
            turns
                .lock()
                .expect("active_turns mutex poisoned")
                .get("thread-1")
                .is_none()
        );
    }

    #[test]
    fn tracks_active_turn_from_nested_turn_payload() {
        let turns = active_turns();

        update_active_turns(
            &turns,
            "turn/started",
            &json!({
                "turn": {
                    "id": "turn-2",
                    "threadId": "thread-2"
                }
            }),
        );

        assert_eq!(
            turns
                .lock()
                .expect("active_turns mutex poisoned")
                .get("thread-2")
                .map(String::as_str),
            Some("turn-2")
        );
    }
}
