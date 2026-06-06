// HTTP handlers.
//
// The four user-facing routes mirror the storage shape: a paginated
// list of threads (with user/repo/status filters and an `updated_ms`
// cursor for "load more"), a single thread, and that thread's messages.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use loro::ExportMode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    codex_bridge::{self, Delta, TurnOptions},
    db::{Annotation, Backend, BackendUpsert, LoroUpdateMeta, Message, Thread, ThreadFilter},
    state::AppState,
    workspace,
};

#[derive(Debug, Deserialize)]
pub struct ListThreadsQuery {
    pub user: Option<String>,
    pub repo: Option<String>,
    pub status: Option<String>,
    pub search: Option<String>,
    pub limit: Option<u32>,
    pub before: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListThreadsResponse {
    pub threads: Vec<Thread>,
    pub next_before: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ListMessagesQuery {
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct ListMessagesResponse {
    pub messages: Vec<Message>,
}

#[derive(Debug, Serialize)]
pub struct ListBackendsResponse {
    pub backends: Vec<Backend>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertBackendRequest {
    pub id: String,
    pub name: String,
    pub url: String,
    pub source: Option<String>,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub vm_name: Option<String>,
    pub runtime: Option<String>,
    pub status: Option<String>,
}

pub async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

#[derive(Debug, Serialize)]
pub struct WtInfoResponse {
    /// Endpoint URL the client should pass to `new WebTransport(...)`.
    pub wt_url: String,
    /// SHA-256 fingerprint of the server's TLS certificate, lowercase
    /// hex. Browsers consume this through the
    /// `serverCertificateHashes` constructor option, which lets a
    /// self-signed dev cert connect without a CA. The hash rotates at
    /// every server boot because the cert is regenerated on the fly.
    pub cert_sha256_hex: String,
}

/// Publish the WebTransport endpoint and the dev cert hash so the
/// client can build a `new WebTransport(url, { serverCertificateHashes })`
/// call without hard-coding either value.
pub async fn wt_info(State(state): State<AppState>) -> impl IntoResponse {
    match &state.wt_info {
        Some(info) => Json(WtInfoResponse {
            wt_url: info.wt_url.clone(),
            cert_sha256_hex: info.cert_hash_hex.clone(),
        })
        .into_response(),
        // Host-placed engine hosts bind no WebTransport listener, so
        // there is no endpoint to advertise.
        None => (
            StatusCode::NOT_FOUND,
            "webtransport listener is disabled on this server",
        )
            .into_response(),
    }
}

pub async fn list_threads(
    State(state): State<AppState>,
    Query(q): Query<ListThreadsQuery>,
) -> impl IntoResponse {
    let filter = ThreadFilter {
        user: q.user,
        repo: q.repo,
        status: q.status,
        search: q.search,
        limit: q.limit.unwrap_or(50),
        before_updated_ms: q.before,
    };
    let result = {
        let db = state.db.lock().await;
        db.list_threads(&filter)
    };
    match result {
        Ok(threads) => {
            let next_before = threads.last().map(|t| t.updated_ms);
            Json(ListThreadsResponse {
                threads,
                next_before,
            })
            .into_response()
        }
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn get_thread(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let result = {
        let db = state.db.lock().await;
        db.get_thread(&id)
    };
    match result {
        Ok(Some(thread)) => Json(thread).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "thread not found").into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn list_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ListMessagesQuery>,
) -> impl IntoResponse {
    let result = {
        let db = state.db.lock().await;
        db.list_messages(&id, q.limit.unwrap_or(500))
    };
    match result {
        Ok(messages) => Json(ListMessagesResponse { messages }).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn list_backends(State(state): State<AppState>) -> impl IntoResponse {
    let result = {
        let db = state.db.lock().await;
        db.list_backends()
    };
    match result {
        Ok(backends) => Json(ListBackendsResponse { backends }).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn upsert_backend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpsertBackendRequest>,
) -> impl IntoResponse {
    if let Err(response) = authorize_backend_write(&state, &headers) {
        return response;
    }

    let id = req.id.trim();
    let name = req.name.trim();
    let url = req.url.trim().trim_end_matches('/');
    if id.is_empty() {
        return (StatusCode::BAD_REQUEST, "backend id is empty").into_response();
    }
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "backend name is empty").into_response();
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return (
            StatusCode::BAD_REQUEST,
            "backend url must use http or https",
        )
            .into_response();
    }

    let result = {
        let db = state.db.lock().await;
        db.upsert_backend(&BackendUpsert {
            id: id.to_owned(),
            name: name.to_owned(),
            url: url.to_owned(),
            source: req.source.unwrap_or_else(|| "external".to_owned()),
            run_id: non_empty(req.run_id),
            node_id: non_empty(req.node_id),
            vm_name: non_empty(req.vm_name),
            runtime: non_empty(req.runtime),
            status: req.status.unwrap_or_else(|| "active".to_owned()),
            now_ms: chrono_like_now_ms(),
        })
    };
    match result {
        Ok(backend) => Json(backend).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn delete_backend(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(response) = authorize_backend_write(&state, &headers) {
        return response;
    }

    let result = {
        let db = state.db.lock().await;
        db.delete_backend(&id, chrono_like_now_ms())
    };
    match result {
        Ok(Some(backend)) => Json(backend).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "backend not found").into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn proxy_list_threads(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ListThreadsQuery>,
) -> impl IntoResponse {
    let Some(base) = backend_url(&state, &id).await else {
        return (StatusCode::NOT_FOUND, "backend not found").into_response();
    };
    proxy_get_json(&base, "/api/threads", thread_query(&q)).await
}

pub async fn proxy_get_thread(
    State(state): State<AppState>,
    Path((id, thread_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(base) = backend_url(&state, &id).await else {
        return (StatusCode::NOT_FOUND, "backend not found").into_response();
    };
    proxy_get_json(
        &base,
        &format!("/api/threads/{}", percent_encode(&thread_id)),
        Vec::new(),
    )
    .await
}

pub async fn proxy_list_messages(
    State(state): State<AppState>,
    Path((id, thread_id)): Path<(String, String)>,
    Query(q): Query<ListMessagesQuery>,
) -> impl IntoResponse {
    let Some(base) = backend_url(&state, &id).await else {
        return (StatusCode::NOT_FOUND, "backend not found").into_response();
    };
    let mut query = Vec::new();
    if let Some(limit) = q.limit {
        query.push(("limit".to_owned(), limit.to_string()));
    }
    proxy_get_json(
        &base,
        &format!("/api/threads/{}/messages", percent_encode(&thread_id)),
        query,
    )
    .await
}

pub async fn proxy_archive_thread(
    State(state): State<AppState>,
    Path((id, thread_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(base) = backend_url(&state, &id).await else {
        return (StatusCode::NOT_FOUND, "backend not found").into_response();
    };
    proxy_post_json(
        &base,
        &format!("/api/threads/{}/archive", percent_encode(&thread_id)),
    )
    .await
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    /// Optional thread id. When absent (or empty), the server creates
    /// a new codex thread, stores it under its real id, and returns
    /// that id in the response so the client can navigate to it.
    pub thread_id: Option<String>,
    #[serde(default)]
    pub text: String,
    pub cwd: Option<String>,
    pub author_id: Option<String>,
    pub author_name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub approval_policy: Option<Value>,
    pub permissions: Option<String>,
    #[serde(default)]
    pub input: Vec<Value>,
    /// Optional inline image attachments encoded as `data:` URLs
    /// (e.g. `data:image/png;base64,...`). Forwarded to codex as
    /// `{"type": "image", "url"}` input items in the order received.
    #[serde(default)]
    pub images: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub thread_id: String,
}

#[derive(Debug, Deserialize)]
pub struct WorkflowTurnRequest {
    pub thread_id: Option<String>,
    pub text: String,
    pub cwd: Option<String>,
    pub run_id: Option<String>,
    pub node_id: Option<String>,
    pub author_id: Option<String>,
    pub author_name: Option<String>,
    #[serde(flatten)]
    pub options: TurnOptions,
}

#[derive(Debug, Serialize)]
pub struct WorkflowTurnResponse {
    pub thread_id: String,
}

#[derive(Debug, Serialize)]
pub struct InterruptThreadResponse {
    pub thread_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequestsResponse {
    pub requests: Vec<crate::codex_rpc::PendingServerRequest>,
}

/// Submit a user message as a real Codex turn. Records the prompt,
/// dispatches `turn/start` over JSON-RPC, and returns the thread id;
/// agent items stream back through the WebSocket delta channel as
/// the codex bridge picks them up.
pub async fn chat(
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    let trimmed = req.text.trim();
    if trimmed.is_empty() && req.images.is_empty() && req.input.is_empty() {
        return (StatusCode::BAD_REQUEST, "input is empty").into_response();
    }
    if trimmed.len() > 16_000 {
        return (StatusCode::PAYLOAD_TOO_LARGE, "text too long").into_response();
    }

    // Conservative caps: anything larger is almost certainly the
    // user accidentally dragging a video or a folder of photos.
    // The data-URL wire format is base64, so a 24 MiB raw image
    // becomes ~32 MiB on the wire — pick limits in that envelope.
    const MAX_IMAGES_PER_MESSAGE: usize = 8;
    const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
    if req.images.len() > MAX_IMAGES_PER_MESSAGE {
        return (StatusCode::PAYLOAD_TOO_LARGE, "too many images").into_response();
    }
    for url in &req.images {
        if !url.starts_with("data:image/") {
            return (
                StatusCode::BAD_REQUEST,
                "image url must be a data:image/* URL",
            )
                .into_response();
        }
        if url.len() > MAX_IMAGE_BYTES {
            return (StatusCode::PAYLOAD_TOO_LARGE, "image too large").into_response();
        }
    }
    for item in &req.input {
        if let Some(url) = item
            .get("url")
            .and_then(Value::as_str)
            .filter(|_| item.get("type").and_then(Value::as_str) == Some("image"))
        {
            if !url.starts_with("data:image/") {
                return (
                    StatusCode::BAD_REQUEST,
                    "image input url must be a data:image/* URL",
                )
                    .into_response();
            }
            if url.len() > MAX_IMAGE_BYTES {
                return (StatusCode::PAYLOAD_TOO_LARGE, "image input too large").into_response();
            }
        }
    }

    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };

    let author = req
        .author_name
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("anon");
    let author_id = req.author_id.as_deref().unwrap_or("anon");
    let author_label = format!("{author}@{author_id}");

    let thread_id_opt = req
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    match codex_bridge::submit_user_turn(
        &codex,
        &state.db,
        &state.broadcast,
        thread_id_opt,
        trimmed,
        &req.images,
        &req.input,
        req.cwd.as_deref(),
        &author_label,
        &TurnOptions {
            model: req.model,
            effort: req.effort,
            approval_policy: req.approval_policy,
            permission_profile: req.permissions,
            ..TurnOptions::default()
        },
    )
    .await
    {
        Ok(thread_id) => Json(ChatResponse { thread_id }).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn workflow_turn(
    State(state): State<AppState>,
    Json(req): Json<WorkflowTurnRequest>,
) -> impl IntoResponse {
    let trimmed = req.text.trim();
    if trimmed.is_empty() {
        return (StatusCode::BAD_REQUEST, "text is empty").into_response();
    }
    if trimmed.len() > 256_000 {
        return (StatusCode::PAYLOAD_TOO_LARGE, "text too long").into_response();
    }

    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };

    let author = req
        .author_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("symphony");
    let author_id = req
        .author_id
        .as_deref()
        .or(req.run_id.as_deref())
        .unwrap_or("workflow");
    let node = req.node_id.as_deref().unwrap_or("node");
    let author_label = format!("{author}@{author_id}/{node}");
    let thread_id_opt = req
        .thread_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    match codex_bridge::submit_workflow_turn(
        &codex,
        &state.db,
        &state.broadcast,
        thread_id_opt,
        trimmed,
        req.cwd.as_deref(),
        &author_label,
        req.options,
    )
    .await
    {
        Ok(thread_id) => Json(WorkflowTurnResponse { thread_id }).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn interrupt_thread(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };

    match codex.interrupt_active_turn(&id).await {
        Ok(()) => Json(InterruptThreadResponse { thread_id: id }).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn pending_codex_requests(State(state): State<AppState>) -> impl IntoResponse {
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };

    Json(PendingRequestsResponse {
        requests: codex.pending_server_requests(),
    })
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct CodexCwdQuery {
    pub cwd: Option<String>,
}

pub async fn codex_models(State(state): State<AppState>) -> impl IntoResponse {
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };
    match codex
        .request("model/list", json!({ "includeHidden": false }))
        .await
    {
        Ok(value) => Json(value).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn codex_permission_profiles(
    State(state): State<AppState>,
    Query(q): Query<CodexCwdQuery>,
) -> impl IntoResponse {
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };
    let mut params = serde_json::Map::new();
    if let Some(cwd) = q.cwd.filter(|s| !s.trim().is_empty()) {
        params.insert("cwd".to_owned(), Value::String(cwd));
    }
    match codex
        .request("permissionProfile/list", Value::Object(params))
        .await
    {
        Ok(value) => Json(value).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn codex_config(
    State(state): State<AppState>,
    Query(q): Query<CodexCwdQuery>,
) -> impl IntoResponse {
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };
    let mut params = serde_json::Map::new();
    params.insert("includeLayers".to_owned(), Value::Bool(false));
    if let Some(cwd) = q.cwd.filter(|s| !s.trim().is_empty()) {
        params.insert("cwd".to_owned(), Value::String(cwd));
    }
    match codex.request("config/read", Value::Object(params)).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn codex_skills(
    State(state): State<AppState>,
    Query(q): Query<CodexCwdQuery>,
) -> impl IntoResponse {
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };
    let mut params = serde_json::Map::new();
    if let Some(cwd) = q.cwd.filter(|s| !s.trim().is_empty()) {
        params.insert("cwds".to_owned(), json!([cwd]));
    }
    match codex.request("skills/list", Value::Object(params)).await {
        Ok(value) => Json(value).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct FileSearchRequest {
    pub query: String,
    pub cwd: Option<String>,
    #[serde(default)]
    pub roots: Vec<String>,
    pub cancellation_token: Option<String>,
}

pub async fn codex_file_search(
    State(state): State<AppState>,
    Json(req): Json<FileSearchRequest>,
) -> impl IntoResponse {
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };
    let roots = if req.roots.is_empty() {
        match workspace::snapshot(req.cwd.as_deref()).await {
            Ok(snap) => vec![snap.root],
            Err(err) => return internal_error(err).into_response(),
        }
    } else {
        req.roots
    };
    match codex
        .request(
            "fuzzyFileSearch",
            json!({
                "query": req.query,
                "roots": roots,
                "cancellationToken": req.cancellation_token,
            }),
        )
        .await
    {
        Ok(value) => Json(value).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct SetGoalRequest {
    /// Trimmed and rejected if empty.
    pub objective: String,
    /// Optional token budget the agent works against; codex tracks
    /// `tokens_used` itself and stops or warns when the budget runs
    /// out. Null leaves the budget unset.
    pub token_budget: Option<i64>,
}

/// Set or replace the goal on a thread. Dispatches `thread/goal/set`
/// to codex; codex echoes a `thread/goal/updated` notification that
/// the bridge persists and broadcasts. We return immediately after
/// the RPC ack so the client sees a fast 200 and the broadcast
/// arrives moments later.
pub async fn set_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SetGoalRequest>,
) -> impl IntoResponse {
    let objective = req.objective.trim();
    if objective.is_empty() {
        return (StatusCode::BAD_REQUEST, "objective is empty").into_response();
    }
    if objective.len() > 4_000 {
        return (StatusCode::PAYLOAD_TOO_LARGE, "objective too long").into_response();
    }
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };
    match codex_bridge::submit_goal_set(&codex, &id, objective, req.token_budget).await {
        Ok(()) => (StatusCode::ACCEPTED, "").into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

/// Clear the goal on a thread. Mirrors `set_goal`.
pub async fn clear_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };
    match codex_bridge::submit_goal_clear(&codex, &id).await {
        Ok(()) => (StatusCode::ACCEPTED, "").into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct CodexRequestResponse {
    /// Exact JSON-RPC result payload expected by the app-server request.
    /// For approvals this is usually a decision object; dynamic tools
    /// and elicitations use their own app-server result shape.
    pub result: serde_json::Value,
}

/// Respond to a Codex server-initiated request that was recorded into
/// the shared Loro document by the bridge.
pub async fn respond_codex_request(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<CodexRequestResponse>,
) -> impl IntoResponse {
    let Some(codex) = state.codex.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "codex backend not configured on this server",
        )
            .into_response();
    };
    match codex.reply_server_request(id, req.result) {
        Ok(()) => (StatusCode::ACCEPTED, "").into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

/// Mark a thread as archived. The Tauri client uses this from the
/// sidebar (`e` over a cursor row) so the user can sweep finished
/// chats out of the active list without losing the transcript.
pub async fn archive_thread(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let now_ms = chrono_like_now_ms();
    let updated = {
        let db = state.db.lock().await;
        db.set_thread_status(&id, "archived", now_ms)
    };
    match updated {
        Ok(Some(thread)) => {
            let _ = state.broadcast.send(Delta::ThreadUpsert {
                thread: thread.clone(),
            });
            let _ = state.broadcast.send(Delta::ThreadArchive {
                thread_id: thread.id.clone(),
            });
            Json(thread).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "thread not found").into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct WorkspacePathQuery {
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChangedFilesResponse {
    pub files: Vec<workspace::ChangedFile>,
}

#[derive(Debug, Serialize)]
pub struct DiffResponse {
    pub diff: String,
}

#[derive(Debug, Serialize)]
pub struct FilesResponse {
    pub root: String,
    pub path: String,
    pub entries: Vec<workspace::FileEntry>,
}

#[derive(Debug, Serialize)]
pub struct FileResponse {
    pub root: String,
    pub path: String,
    pub contents: String,
    pub truncated: bool,
}

pub async fn thread_workspace(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match resolve_thread_workspace(&state, &id).await {
        Ok((snapshot, _base_sha)) => Json(snapshot).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn thread_changed_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match resolve_thread_workspace(&state, &id).await {
        Ok((snapshot, base_sha)) => {
            match workspace::changed_files(&snapshot.root, base_sha.as_deref()).await {
                Ok(files) => Json(ChangedFilesResponse { files }).into_response(),
                Err(err) => internal_error(err).into_response(),
            }
        }
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn thread_diff(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<WorkspacePathQuery>,
) -> impl IntoResponse {
    match resolve_thread_workspace(&state, &id).await {
        Ok((snapshot, base_sha)) => {
            match workspace::diff(&snapshot.root, base_sha.as_deref(), q.path.as_deref()).await {
                Ok(diff) => Json(DiffResponse { diff }).into_response(),
                Err(err) => internal_error(err).into_response(),
            }
        }
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn thread_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<WorkspacePathQuery>,
) -> impl IntoResponse {
    match resolve_thread_workspace(&state, &id).await {
        Ok((snapshot, _base_sha)) => {
            let path = q.path.unwrap_or_default();
            match workspace::list_files(&snapshot.root, Some(&path)) {
                Ok(entries) => Json(FilesResponse {
                    root: snapshot.root,
                    path,
                    entries,
                })
                .into_response(),
                Err(err) => internal_error(err).into_response(),
            }
        }
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn thread_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<WorkspacePathQuery>,
) -> impl IntoResponse {
    let Some(path) = q.path.filter(|s| !s.trim().is_empty()) else {
        return (StatusCode::BAD_REQUEST, "path is required").into_response();
    };
    match resolve_thread_workspace(&state, &id).await {
        Ok((snapshot, _base_sha)) => match workspace::read_file(&snapshot.root, &path) {
            Ok(file) => Json(FileResponse {
                root: snapshot.root,
                path,
                contents: file.contents,
                truncated: file.truncated,
            })
            .into_response(),
            Err(err) => internal_error(err).into_response(),
        },
        Err(err) => internal_error(err).into_response(),
    }
}

async fn resolve_thread_workspace(
    state: &AppState,
    id: &str,
) -> anyhow::Result<(workspace::WorkspaceSnapshot, Option<String>)> {
    let thread = {
        let db = state.db.lock().await;
        db.get_thread(id)?
    }
    .ok_or_else(|| anyhow::anyhow!("thread not found"))?;

    let snapshot = if let Some(root) = thread.workspace_root.clone() {
        workspace::WorkspaceSnapshot {
            cwd: thread.cwd.clone().unwrap_or_else(|| root.clone()),
            root,
            repo: thread.repo.clone(),
            branch: thread.branch.clone(),
            base_sha: thread.base_sha.clone(),
        }
    } else {
        workspace::snapshot(thread.cwd.as_deref()).await?
    };
    let base_sha = thread.base_sha.or_else(|| snapshot.base_sha.clone());
    Ok((snapshot, base_sha))
}

fn chrono_like_now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn internal_error(err: anyhow::Error) -> (StatusCode, String) {
    eprintln!("room: 500 {err:#}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("server error: {err:#}"),
    )
}

fn authorize_backend_write(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), axum::response::Response> {
    let Some(expected) = state.backend_token.as_deref() else {
        return Ok(());
    };
    let got = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match got {
        Some(token) if token == expected => Ok(()),
        _ => Err((StatusCode::UNAUTHORIZED, "missing or invalid backend token").into_response()),
    }
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

async fn backend_url(state: &AppState, id: &str) -> Option<String> {
    let db = state.db.lock().await;
    db.get_backend(id)
        .ok()
        .flatten()
        .filter(|backend| backend.status == "active")
        .map(|backend| backend.url)
}

async fn proxy_get_json(
    base: &str,
    path: &str,
    query: Vec<(String, String)>,
) -> axum::response::Response {
    let client = reqwest::Client::new();
    let mut request = client.get(format!("{}{}", base.trim_end_matches('/'), path));
    if !query.is_empty() {
        request = request.query(&query);
    }
    match request.send().await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    }
}

async fn proxy_post_json(base: &str, path: &str) -> axum::response::Response {
    let client = reqwest::Client::new();
    match client
        .post(format!("{}{}", base.trim_end_matches('/'), path))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            (
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(err) => (StatusCode::BAD_GATEWAY, err.to_string()).into_response(),
    }
}

fn thread_query(q: &ListThreadsQuery) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Some(user) = &q.user {
        out.push(("user".to_owned(), user.clone()));
    }
    if let Some(repo) = &q.repo {
        out.push(("repo".to_owned(), repo.clone()));
    }
    if let Some(status) = &q.status {
        out.push(("status".to_owned(), status.clone()));
    }
    if let Some(search) = &q.search {
        out.push(("search".to_owned(), search.clone()));
    }
    if let Some(limit) = q.limit {
        out.push(("limit".to_owned(), limit.to_string()));
    }
    if let Some(before) = q.before {
        out.push(("before".to_owned(), before.to_string()));
    }
    out
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct ListLoroUpdatesQuery {
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct LoroStateResponse {
    /// Materialized doc value — the same shape the JS side gets from
    /// `doc.toJSON()`. Suitable for `jq`-style poking; not stable
    /// wire shape.
    pub state: serde_json::Value,
    /// Total rows in the durable update log. Cheap "is the server
    /// actually persisting?" signal.
    pub log_rows: i64,
}

#[derive(Debug, Serialize)]
pub struct LoroUpdatesResponse {
    pub updates: Vec<LoroUpdateMeta>,
}

#[derive(Debug, Deserialize)]
pub struct ListAnnotationsQuery {
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct ListAnnotationsResponse {
    pub annotations: Vec<Annotation>,
}

/// Read the SQL mirror of reviewer notes. Latest-first; the limit
/// is capped server-side. This is the entry point operators are
/// expected to `curl | jq` against when they sit down to mine
/// flagged turns for AGENTS.md improvements.
pub async fn list_annotations(
    State(state): State<AppState>,
    Query(q): Query<ListAnnotationsQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(200);
    let result = {
        let db = state.db.lock().await;
        db.list_annotations(limit)
    };
    match result {
        Ok(annotations) => Json(ListAnnotationsResponse { annotations }).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

pub async fn loro_state(State(state): State<AppState>) -> impl IntoResponse {
    let value = {
        let doc = state.loro_doc.lock().await;
        doc.get_deep_value()
    };
    let log_rows = {
        let db = state.db.lock().await;
        match db.recent_loro_update_meta(1) {
            Ok(rows) => rows.first().map(|r| r.seq).unwrap_or(0),
            Err(err) => return internal_error(err).into_response(),
        }
    };
    let json = match serde_json::to_value(value) {
        Ok(v) => v,
        Err(err) => return internal_error(err.into()).into_response(),
    };
    Json(LoroStateResponse {
        state: json,
        log_rows,
    })
    .into_response()
}

pub async fn loro_updates(
    State(state): State<AppState>,
    Query(q): Query<ListLoroUpdatesQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let result = {
        let db = state.db.lock().await;
        db.recent_loro_update_meta(limit)
    };
    match result {
        Ok(updates) => Json(LoroUpdatesResponse { updates }).into_response(),
        Err(err) => internal_error(err).into_response(),
    }
}

/// Raw snapshot bytes for an external client (a CLI dumper, another
/// loro doc that wants to hydrate). Matches the wire shape the WS
/// handler sends to clients on connect.
pub async fn loro_snapshot(State(state): State<AppState>) -> impl IntoResponse {
    let bytes = {
        let doc = state.loro_doc.lock().await;
        doc.export(ExportMode::Snapshot)
    };
    match bytes {
        Ok(bytes) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(err) => internal_error(anyhow::anyhow!("export snapshot: {err:?}")).into_response(),
    }
}
