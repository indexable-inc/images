//! `screencast-ingest`: receive H.265 HLS screen streams and keep them.
//!
//! The `screencast` client pushes a fragmented-MP4 HLS stream by `PUT`-ing the
//! init segment, each media segment, and the rolling playlist to
//! `/ingest/{user}/{session}/{file}`. This server writes those files under a
//! root directory, one folder per user and session, and serves them back so the
//! same URLs play in any HLS client (Safari natively, others via hls.js). The
//! filesystem is the single source of truth: the session index and dashboard
//! are derived from what is on disk, nothing is tracked separately.
//!
//! Everything is retained. A finished session is a complete VOD ready to feed
//! into downstream data and context pipelines (frame sampling, OCR, indexing);
//! a live session is the same files mid-write.
//!
//! Cross-platform, so it deploys on the Linux fleet. There is no transcoding:
//! segments are stored exactly as the hardware encoder produced them.

use std::cmp::Reverse;
use std::net::SocketAddr;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, put};
use clap::Parser;
use color_eyre::eyre::{Result, WrapErr};
use serde::Serialize;
use tracing::{info, warn};

/// Largest single upload accepted. A segment is bitrate times segment length; 64
/// MiB covers a long, high-bitrate segment (e.g. 10s at 50 Mbit/s) with room to
/// spare while bounding the memory a single request can pin.
const MAX_UPLOAD: usize = 64 * 1024 * 1024;

/// A session is considered live if its newest file changed within this window.
const LIVE_WINDOW_SECS: u64 = 30;

/// How long a `/api/sessions` filesystem scan is reused. Bounds the cost of the
/// dashboard's polling (many open tabs share one scan) and of unauthenticated
/// callers, so a poll storm cannot amplify into a per-request full-tree walk.
const SCAN_CACHE_TTL: Duration = Duration::from_secs(2);

/// Ingest and serve H.265 HLS screen streams.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Directory under which streams are stored as `{user}/{session}/...`.
    #[arg(long, env = "SCREENCAST_ROOT", default_value = "./screencast-data")]
    root: PathBuf,

    /// Address to listen on.
    #[arg(long, env = "SCREENCAST_ADDR", default_value = "0.0.0.0:8080")]
    addr: SocketAddr,

    /// If set, uploads must carry `Authorization: Bearer <token>`. Playback and
    /// the dashboard stay open; this guards writes, not reads.
    #[arg(long, env = "SCREENCAST_TOKEN")]
    token: Option<String>,
}

#[derive(Clone)]
struct AppState(Arc<Inner>);

struct Inner {
    root: PathBuf,
    token: Option<String>,
    scan: Mutex<ScanCache>,
}

/// Short-lived cache of the last `/api/sessions` scan.
#[derive(Default)]
struct ScanCache {
    at: Option<Instant>,
    data: Arc<Vec<SessionInfo>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    tokio::fs::create_dir_all(&args.root)
        .await
        .wrap_err_with(|| format!("creating root {}", args.root.display()))?;
    let root = args
        .root
        .canonicalize()
        .wrap_err_with(|| format!("resolving root {}", args.root.display()))?;
    if args.token.is_some() {
        info!("upload authentication enabled");
    }
    let state = AppState(Arc::new(Inner {
        root: root.clone(),
        token: args.token,
        scan: Mutex::new(ScanCache::default()),
    }));

    let app = Router::new()
        .route("/", get(dashboard))
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/sessions", get(api_sessions))
        .route(
            "/ingest/{user}/{session}/{file}",
            put(upload).get(serve).delete(remove),
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(args.addr)
        .await
        .wrap_err_with(|| format!("binding {}", args.addr))?;
    info!(addr = %args.addr, root = %root.display(), "screencast-ingest listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutting down");
        })
        .await
        .wrap_err("server error")
}

/// A single path segment is safe when it is a plain name: non-empty, not a
/// directory-traversal token, and built only from the conservative charset the
/// client also sanitizes to. This is the sole guard standing between a client
/// path and a write, so it rejects anything it does not positively recognize.
fn safe_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s != "."
        && s != ".."
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Map a stored file's extension to its HLS content type.
fn content_type(file: &str) -> &'static str {
    match file.rsplit('.').next() {
        Some("m3u8") => "application/vnd.apple.mpegurl",
        Some("m4s" | "mp4") => "video/mp4",
        Some("ts") => "video/mp2t",
        _ => "application/octet-stream",
    }
}

/// An HTTP error response: a status code and a client-safe message. Returned as
/// the `Err` of handlers and helpers so the `(StatusCode, String)` axum response
/// tuple is built from a named, self-documenting shape rather than a bare pair.
struct HttpError {
    status: StatusCode,
    message: String,
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

/// A validated on-disk location for an ingest file: the session directory and
/// the full path to the file within it.
struct ResolvedPath {
    dir: PathBuf,
    path: PathBuf,
}

/// Resolve `{user}/{session}/{file}` to an on-disk path, rejecting any segment
/// that is not a safe plain name. Returns the validated directory and full path.
fn resolve(root: &FsPath, user: &str, session: &str, file: &str) -> Result<ResolvedPath, HttpError> {
    if !(safe_component(user) && safe_component(session) && safe_component(file)) {
        return Err(HttpError {
            status: StatusCode::BAD_REQUEST,
            message: "invalid path component".to_owned(),
        });
    }
    let dir = root.join(user).join(session);
    let path = dir.join(file);
    Ok(ResolvedPath { dir, path })
}

/// Enforce the bearer token on a mutating request when one is configured. The
/// token compare is constant-time so a timing side channel cannot recover it
/// byte by byte (the length is allowed to leak, which is conventional).
fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<(), HttpError> {
    let Some(expected) = &state.0.token else {
        return Ok(());
    };
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if presented.is_some_and(|p| ct_eq(p.as_bytes(), expected.as_bytes())) {
        Ok(())
    } else {
        Err(HttpError {
            status: StatusCode::UNAUTHORIZED,
            message: "missing or invalid bearer token".to_owned(),
        })
    }
}

/// Constant-time byte-slice equality (short-circuits only on length).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// `PUT` a playlist or segment. Auth is checked before the body is read, so an
/// unauthenticated request never buffers a large upload. The body is written to
/// a temp file in the destination directory and renamed into place, so a reader
/// (a player polling the playlist) never observes a half-written file.
async fn upload(
    State(state): State<AppState>,
    Path((user, session, file)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<StatusCode, HttpError> {
    check_auth(&state, &headers)?;
    let ResolvedPath { dir, path } = resolve(&state.0.root, &user, &session, &file)?;
    if !matches!(file.rsplit('.').next(), Some("m3u8" | "m4s" | "mp4" | "ts")) {
        return Err(HttpError {
            status: StatusCode::BAD_REQUEST,
            message: "unsupported file type".to_owned(),
        });
    }

    let bytes = to_bytes(body, MAX_UPLOAD).await.map_err(|_| HttpError {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        message: "upload too large".to_owned(),
    })?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| internal(&format!("create dir: {e}")))?;
    let tmp = dir.join(format!(".{file}.tmp"));
    tokio::fs::write(&tmp, &bytes)
        .await
        .map_err(|e| internal(&format!("write: {e}")))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| internal(&format!("rename: {e}")))?;
    Ok(StatusCode::CREATED)
}

/// `GET` a stored playlist or segment for playback.
async fn serve(
    State(state): State<AppState>,
    Path((user, session, file)): Path<(String, String, String)>,
) -> Response {
    let path = match resolve(&state.0.root, &user, &session, &file) {
        Ok(resolved) => resolved.path,
        Err(e) => return e.into_response(),
    };
    tokio::fs::read(&path).await.map_or_else(
        |_| (StatusCode::NOT_FOUND, "not found").into_response(),
        |bytes| {
            let ct = HeaderValue::from_static(content_type(&file));
            ([(header::CONTENT_TYPE, ct)], bytes).into_response()
        },
    )
}

/// `DELETE` a stored file. Supports HLS muxers configured to expire old
/// segments; the default client keeps everything, so this is rarely used.
async fn remove(
    State(state): State<AppState>,
    Path((user, session, file)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, HttpError> {
    check_auth(&state, &headers)?;
    let path = resolve(&state.0.root, &user, &session, &file)?.path;
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(internal(&format!("remove: {e}"))),
    }
}

/// Log the detailed cause and return a generic 500, so an OS error string
/// (which can include absolute server paths) never reaches the client.
fn internal(detail: &str) -> HttpError {
    warn!("{detail}");
    HttpError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "internal error".to_owned(),
    }
}

/// One stream's summary, derived entirely from its directory on disk.
#[derive(Clone, Debug, Serialize)]
struct SessionInfo {
    user: String,
    session: String,
    /// URL of the HLS playlist for this session.
    playlist: String,
    /// Earliest file mtime (epoch seconds): when capture began.
    started: u64,
    /// Latest file mtime (epoch seconds): last write seen.
    updated: u64,
    /// Number of media segments stored.
    segments: u64,
    /// Total bytes on disk for the session.
    bytes: u64,
    /// True if the session was written within the live window.
    live: bool,
    /// True once the playlist carries `#EXT-X-ENDLIST` (a finished VOD).
    complete: bool,
}

/// Scan the root directory and summarize every session. Errors on individual
/// entries are skipped rather than failing the whole listing, so one unreadable
/// folder cannot blank the dashboard.
async fn scan_sessions(root: &FsPath) -> Vec<SessionInfo> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut out = Vec::new();

    let Ok(mut users) = tokio::fs::read_dir(root).await else {
        return out;
    };
    while let Ok(Some(user_ent)) = users.next_entry().await {
        if !user_ent.file_type().await.is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let user = user_ent.file_name().to_string_lossy().into_owned();
        let Ok(mut sessions) = tokio::fs::read_dir(user_ent.path()).await else {
            continue;
        };
        while let Ok(Some(sess_ent)) = sessions.next_entry().await {
            if !sess_ent.file_type().await.is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let session = sess_ent.file_name().to_string_lossy().into_owned();
            if let Some(info) = summarize(sess_ent.path(), &user, &session, now).await {
                out.push(info);
            }
        }
    }
    // Most recently active first.
    out.sort_by_key(|s| Reverse(s.updated));
    out
}

/// Summarize one session directory, or `None` if it has no playlist yet.
async fn summarize(dir: PathBuf, user: &str, session: &str, now: u64) -> Option<SessionInfo> {
    let mut started = u64::MAX;
    let mut updated = 0u64;
    let mut segments = 0u64;
    let mut bytes = 0u64;
    let mut has_playlist = false;

    let mut entries = tokio::fs::read_dir(&dir).await.ok()?;
    while let Ok(Some(ent)) = entries.next_entry().await {
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // in-flight temp file
        }
        let Ok(meta) = ent.metadata().await else { continue };
        if !meta.is_file() {
            continue;
        }
        bytes += meta.len();
        let ext = name.rsplit('.').next().unwrap_or_default();
        if ext.eq_ignore_ascii_case("m4s") || ext.eq_ignore_ascii_case("ts") {
            segments += 1;
        }
        if name == "index.m3u8" {
            has_playlist = true;
        }
        if let Ok(modified) = meta.modified()
            && let Ok(since_epoch) = modified.duration_since(UNIX_EPOCH)
        {
            let secs = since_epoch.as_secs();
            started = started.min(secs);
            updated = updated.max(secs);
        }
    }

    if !has_playlist {
        return None;
    }
    let complete = tokio::fs::read_to_string(dir.join("index.m3u8"))
        .await
        .is_ok_and(|p| p.contains("#EXT-X-ENDLIST"));

    Some(SessionInfo {
        user: user.to_owned(),
        session: session.to_owned(),
        playlist: format!("/ingest/{user}/{session}/index.m3u8"),
        started: if started == u64::MAX { updated } else { started },
        updated,
        segments,
        bytes,
        live: !complete && now.saturating_sub(updated) <= LIVE_WINDOW_SECS,
        complete,
    })
}

/// JSON list of all sessions, for the dashboard and downstream consumers. The
/// scan is cached for `SCAN_CACHE_TTL` so polling (many dashboard tabs, or an
/// unauthenticated caller) cannot turn each request into a full-tree walk.
async fn api_sessions(State(state): State<AppState>) -> Json<Vec<SessionInfo>> {
    let fresh = state.0.scan.lock().ok().and_then(|cache| {
        (cache.at.is_some_and(|at| at.elapsed() < SCAN_CACHE_TTL)).then(|| (*cache.data).clone())
    });
    if let Some(data) = fresh {
        return Json(data);
    }
    let data = scan_sessions(&state.0.root).await;
    if let Ok(mut cache) = state.0.scan.lock() {
        cache.at = Some(Instant::now());
        cache.data = Arc::new(data.clone());
    }
    Json(data)
}

/// The single-page dashboard. It fetches `/api/sessions` itself, so the HTML is
/// fully static and needs no server-side templating.
async fn dashboard() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}
