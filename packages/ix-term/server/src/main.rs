//! ix-term server: a tailnet-internal web terminal (index#3797).
//!
//! One server-side libghostty-vt state per session is the single source of
//! truth; browsers are thin views over a dirty-row websocket protocol. PTYs
//! are spawned on the serving host; the `ixterm` CLI opens HTML files in a
//! session by writing OSC 5522 to the session pts.
//!
//! Auth: the server binds localhost and trusts its reverse proxy / tailnet
//! (term.ix.dev terminates on tailscale). Tailscale `WhoIs` identity per
//! request is a documented TODO; there is deliberately no login UI.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use clap::{Parser, Subcommand};
use tower_http::services::ServeDir;
use uuid::Uuid;

mod osc;
mod proto;
mod session;
mod vt;
mod ws;

use session::{ServerConfig, SessionManager};

#[derive(Parser)]
#[command(about = "Tailnet-internal web terminal on server-side libghostty-vt")]
struct Args {
    /// Address to bind. Defaults to loopback: the tailnet reverse proxy is
    /// the trust boundary, so the server itself never listens publicly.
    #[arg(long, env = "IX_TERM_LISTEN", default_value = "127.0.0.1:7533")]
    listen: SocketAddr,

    /// Static UI directory. The Nix package wrapper sets this; unset means
    /// API-only (useful in development with `vite dev` proxying).
    #[arg(long, env = "IX_TERM_SITE_DIR")]
    site_dir: Option<PathBuf>,

    /// Runtime directory for the `ixterm` CLI contract
    /// (`<dir>/sessions/<id>/pts`).
    #[arg(long, env = "IX_TERM_RUNTIME_DIR", default_value = "/run/ix-term")]
    runtime_dir: PathBuf,

    /// Shell for new sessions; defaults to `$SHELL` then `/bin/sh`.
    #[arg(long, env = "IX_TERM_SHELL")]
    shell: Option<String>,

    /// Scrollback lines kept per session.
    #[arg(long, default_value_t = 10_000)]
    scrollback: usize,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Measure the dirty-row pipeline under a synthetic redraw storm and
    /// print rows/sec (the index#3797 pre-freeze benchmark).
    BenchFlood {
        /// Grid height.
        #[arg(long, default_value_t = 40)]
        rows: u16,
        /// Grid width.
        #[arg(long, default_value_t = 120)]
        cols: u16,
        /// Measurement duration per phase.
        #[arg(long, default_value_t = 5)]
        seconds: u64,
    },
}

/// Shared handler state.
#[derive(Clone)]
struct App {
    manager: Arc<SessionManager>,
    site_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();
    let args = Args::parse();

    if let Some(Command::BenchFlood {
        rows,
        cols,
        seconds,
    }) = args.command
    {
        return bench_flood(rows, cols, Duration::from_secs(seconds)).await;
    }

    let manager = Arc::new(SessionManager::new(ServerConfig {
        runtime_dir: args.runtime_dir,
        shell: args.shell,
        scrollback: args.scrollback,
    })?);
    let app = App {
        manager: Arc::clone(&manager),
        site_dir: args.site_dir.clone(),
    };

    let router = Router::new()
        .route("/", get(serve_index))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route(
            "/api/sessions/{id}",
            axum::routing::patch(rename_session).delete(close_session),
        )
        .route("/api/sessions/{id}/ws", get(terminal_ws))
        .route("/api/sessions/{id}/doc", get(session_doc))
        .route("/api/ws", get(events_ws));
    let router = match args.site_dir {
        Some(ref dir) => router.fallback_service(ServeDir::new(dir.clone())),
        None => router,
    };
    let router = router.with_state(app);

    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("cannot bind {}", args.listen))?;
    tracing::info!(listen = %args.listen, "ix-term server up");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("server error")
}

/// Serve the SPA entry point with `no-store` so UI deploys take effect on
/// reload (the hashed assets under it stay cacheable via `ServeDir`).
async fn serve_index(State(app): State<App>) -> Response {
    let Some(dir) = app.site_dir else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "ix-term: no site directory configured (IX_TERM_SITE_DIR)",
        )
            .into_response();
    };
    match tokio::fs::read(dir.join("index.html")).await {
        Ok(body) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            body,
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("ix-term: cannot read index.html: {error}"),
        )
            .into_response(),
    }
}

async fn list_sessions(State(app): State<App>) -> Response {
    axum::Json(app.manager.list().await).into_response()
}

/// Body for session create/rename.
#[derive(serde::Deserialize, Default)]
struct NameBody {
    name: Option<String>,
}

async fn create_session(State(app): State<App>, body: Option<axum::Json<NameBody>>) -> Response {
    let name = body.and_then(|b| b.0.name);
    match app.manager.create(name).await {
        Ok(session) => axum::Json(session.meta()).into_response(),
        Err(error) => {
            tracing::error!(%error, "session create failed");
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}")).into_response()
        }
    }
}

/// Parse a session id path segment.
fn parse_id(id: &str) -> Result<Uuid, StatusCode> {
    Uuid::parse_str(id).map_err(|_| StatusCode::BAD_REQUEST)
}

async fn rename_session(
    State(app): State<App>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<NameBody>,
) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return status.into_response(),
    };
    let Some(name) = body.name else {
        return (StatusCode::BAD_REQUEST, "missing name").into_response();
    };
    if app.manager.rename(id, name).await {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn close_session(State(app): State<App>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return status.into_response(),
    };
    if app.manager.close(id).await {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn terminal_ws(
    State(app): State<App>,
    Path(id): Path<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return status.into_response(),
    };
    let Some(session) = app.manager.get(id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    upgrade.on_upgrade(move |socket| ws::terminal_client(session, socket))
}

async fn events_ws(State(app): State<App>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| ws::events_client(app.manager, socket))
}

/// Serve the session's opened HTML document into its sandboxed iframe.
///
/// "Sandboxed, no network in v1": the iframe carries `sandbox="allow-scripts"`
/// (no same-origin), and this response's Content-Security-Policy forbids
/// every fetch the document could make — inline style/script may run, but
/// nothing loads over the network.
async fn session_doc(State(app): State<App>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return status.into_response(),
    };
    let Some(session) = app.manager.get(id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(path) = session.doc_path() else {
        return (StatusCode::NOT_FOUND, "no document opened").into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(body) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; \
                     img-src data:; font-src data:; form-action 'none'; base-uri 'none'",
                ),
            ],
            body,
        )
            .into_response(),
        Err(error) => {
            (StatusCode::NOT_FOUND, format!("cannot read {path}: {error}")).into_response()
        }
    }
}

/// One full-screen synthetic redraw: home the cursor, rewrite every row with
/// per-frame content and a color change, like a busy TUI (btop-ish).
fn flood_frame(frame: u64, rows: u16, cols: u16) -> Vec<u8> {
    use std::fmt::Write as _;
    const LETTERS: &[u8; 26] = b"abcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(usize::from(rows) * (usize::from(cols) + 16));
    out.push_str("\x1b[H");
    for y in 0..rows {
        let color = 31 + ((frame + u64::from(y)) % 7);
        let _ = write!(out, "\x1b[{color}m");
        for x in 0..cols {
            let index = usize::try_from((frame + u64::from(y) + u64::from(x)) % 26)
                .expect("value below 26 fits usize");
            out.push(char::from(LETTERS[index]));
        }
        if y + 1 < rows {
            out.push_str("\r\n");
        }
    }
    out.into_bytes()
}

/// The pre-freeze benchmark (index#3797): drive a synthetic escape flood
/// through the VT engine and the dirty-row pipeline and report rows/sec.
async fn bench_flood(rows: u16, cols: u16, duration: Duration) -> anyhow::Result<()> {
    bench_ingest(rows, cols, duration)?;

    // Phase 2: the full pipeline — engine thread, frame coalescing, dirty-row
    // diff, wire serialization — with a flood producer.
    let (events, mut rx) = tokio::sync::broadcast::channel::<Arc<proto::ServerMsg>>(1024);
    let engine = vt::spawn_engine(rows, cols, 0, events)?;
    let deadline = Instant::now() + duration;
    let producer = tokio::spawn(async move {
        let mut frame: u64 = 0;
        let mut sent_rows: u64 = 0;
        while Instant::now() < deadline {
            engine.send(vt::EngineMsg::Feed(flood_frame(frame, rows, cols)));
            frame += 1;
            sent_rows += u64::from(rows);
            // Pace the producer so the unbounded engine queue stays shallow;
            // 2000 frames/sec of full redraws is far beyond any real TUI.
            if frame.is_multiple_of(16) {
                tokio::time::sleep(Duration::from_millis(8)).await;
            }
        }
        sent_rows
    });

    let mut wire_rows: u64 = 0;
    let mut wire_frames: u64 = 0;
    let mut wire_bytes: u64 = 0;
    let started = Instant::now();
    loop {
        let timeout = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        match timeout {
            Ok(Ok(msg)) => {
                if let proto::ServerMsg::Grid { changed, .. } = msg.as_ref() {
                    wire_rows += u64::try_from(changed.len()).expect("row count fits u64");
                    wire_frames += 1;
                    let frame_len = serde_json::to_string(msg.as_ref()).map_or(0, |s| s.len());
                    wire_bytes += u64::try_from(frame_len).expect("frame length fits u64");
                }
            }
            Ok(Err(_)) | Err(_) => {
                if producer.is_finished() && Instant::now() > deadline {
                    break;
                }
            }
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    let sent_rows = producer.await.unwrap_or(0);
    println!(
        "pipeline: absorbed {sent_rows} input rows ({:.0} rows/sec); \
         shipped {wire_rows} dirty rows in {wire_frames} frames ({:.0} rows/sec, {:.1} fps, {:.0} KiB/s wire)",
        f64_from(sent_rows) / elapsed,
        f64_from(wire_rows) / elapsed,
        f64_from(wire_frames) / elapsed,
        f64_from(wire_bytes) / elapsed / 1024.0
    );
    Ok(())
}

/// Phase 1 of the benchmark: raw VT ingestion (parse + state), no pipeline.
///
/// Separate from [`bench_flood`] because the `!Send` [`ix_vt::Terminal`] must
/// not live across an await point in an async fn.
fn bench_ingest(rows: u16, cols: u16, duration: Duration) -> anyhow::Result<()> {
    let mut terminal = ix_vt::Terminal::new(rows, cols, 0)?;
    let start = Instant::now();
    let mut frames_in: u64 = 0;
    while start.elapsed() < duration {
        terminal.vt_write(&flood_frame(frames_in, rows, cols));
        frames_in += 1;
    }
    let ingest_secs = start.elapsed().as_secs_f64();
    let ingest_rows = frames_in * u64::from(rows);
    println!(
        "ingest: {ingest_rows} rows in {ingest_secs:.2}s = {:.0} rows/sec ({frames_in} frames)",
        f64_from(ingest_rows) / ingest_secs
    );
    Ok(())
}

/// `u64 -> f64` for benchmark reporting (precision loss is irrelevant here,
/// and the fork's `fallible_int_fallback` lint forbids `as`).
const fn f64_from(value: u64) -> f64 {
    #[allow(clippy::cast_precision_loss, reason = "benchmark reporting only")]
    let out = value as f64;
    out
}
