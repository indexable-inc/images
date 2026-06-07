use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use clap::{Parser, ValueEnum};
use nix_web_monitor_parser::{Delta, MonitorSnapshot, MonitorState, NixEvent, ParsedLine, strip_ansi};
use tokio::io::{self, AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::time::timeout;
use tower_http::services::ServeDir;

mod dependencies;
use dependencies::resolve_dependencies;

/// Bound on the delta broadcast ring. A client that falls this far behind gets
/// resynced with a fresh `Reset` rather than the dropped frames.
const DELTA_CHANNEL_CAPACITY: usize = 1024;

/// Cap on a single WebSocket frame send. A client that completes the handshake
/// but then stops reading would otherwise park the per-client task in `send`
/// indefinitely; this bounds that to a drop instead of a permanent pin.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Parser)]
#[command(
    about = "Run a Nix command with quiet terminal output and a live browser monitor.",
    version
)]
#[allow(clippy::struct_field_names)] // `nix_args` is the wire-level name passed to `nix`; renaming would hurt the CLI help text.
struct Args {
    /// Interface used by the web monitor. Defaults to all interfaces so the UI
    /// is reachable over LAN/Tailscale without a flag. The live feed is a plain
    /// WebSocket, so off-host access needs no certificate.
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    /// TCP port for the UI, the JSON snapshot, and the WebSocket delta feed.
    #[arg(long, default_value_t = 7532)]
    port: u16,

    /// Static UI directory. The Nix package wrapper fills this in.
    #[arg(long, env = "NIX_WEB_MONITOR_SITE_DIR")]
    site_dir: PathBuf,

    /// Exit when the Nix command finishes instead of keeping the web UI alive.
    #[arg(long)]
    exit_when_done: bool,

    /// Terminal output policy. The browser always receives the parsed stream.
    #[arg(long, value_enum, default_value_t = TerminalOutput::Summary)]
    terminal_output: TerminalOutput,

    /// Pass `-v` to Nix for richer internal-json activity events.
    #[arg(long)]
    nix_verbose: bool,

    /// Arguments passed to `nix`. Example: `build .#hello --keep-going`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    nix_args: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TerminalOutput {
    /// Print the URL, plain command output, warnings, errors, and final status.
    Summary,

    /// Print parsed build logs and activity messages as well.
    Logs,

    /// Print only wrapper status lines.
    Quiet,
}

/// Shared state for the HTTP handlers: the monitor for one-shot JSON snapshots,
/// the broadcast sender each WebSocket subscribes to for the live feed, and the
/// cached `index.html` bytes served with cache-busting headers.
#[derive(Clone)]
struct AppState {
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
    index_html: Bytes,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    validate_site_dir(&args.site_dir)?;

    let index_html = Bytes::from(
        std::fs::read(args.site_dir.join("index.html")).context("reading index.html")?,
    );

    let monitor = Arc::new(RwLock::new(MonitorState::default()));
    let (deltas, _) = broadcast::channel::<Bytes>(DELTA_CHANNEL_CAPACITY);

    let http_addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .with_context(|| format!("invalid HTTP address {}:{}", args.host, args.port))?;

    let state = AppState {
        monitor: Arc::clone(&monitor),
        deltas: deltas.clone(),
        index_html,
    };
    let http_server = serve(http_addr, args.site_dir, state).await?;

    eprintln!("nix-web-monitor: http://{http_addr}");

    let build = tokio::spawn(run_nix_command(
        args.nix_args,
        args.terminal_output,
        args.nix_verbose,
        monitor,
        deltas,
    ));

    let exit_code = build.await.context("joining Nix command task")??;

    if !args.exit_when_done {
        eprintln!(
            "nix-web-monitor: Nix command finished with {}; press Ctrl-C to stop the web UI",
            exit_code.map_or_else(|| "no exit code".to_owned(), |code| code.to_string())
        );
        tokio::signal::ctrl_c()
            .await
            .context("waiting for Ctrl-C")?;
    }
    http_server.abort();
    // Propagate Nix's exit status either way; otherwise the wrapper masks
    // build failures from shells and CI.
    std::process::exit(exit_code.unwrap_or(1));
}

async fn serve(
    addr: SocketAddr,
    site_dir: PathBuf,
    state: AppState,
) -> Result<tokio::task::JoinHandle<()>> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding web monitor on {addr}"))?;
    let app = router(&site_dir, state);

    Ok(tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("nix-web-monitor: web server failed: {error}");
        }
    }))
}

/// Build the HTTP router: the JSON snapshot, the WebSocket live feed, the
/// cache-busting `index.html`, and the hashed static assets, all on one port.
/// Split from [`serve`] so it can be exercised without binding a socket.
///
/// `/` is served by [`serve_index`] (not `ServeDir`) so it carries `no-store`;
/// every other path falls through to `ServeDir`, which 404s a missing file
/// rather than answering with `index.html`. Returning HTML for a missing
/// `/assets/*.js` is what makes the browser reject it for the wrong MIME type
/// after a rebuild changes the asset hashes.
fn router(site_dir: &Path, state: AppState) -> Router {
    let static_files = ServeDir::new(site_dir);
    Router::new()
        .route("/", get(serve_index))
        .route("/api/state", get(state_snapshot))
        .route("/ws", get(ws_handler))
        .fallback_service(static_files)
        .with_state(state)
}

/// Serve `index.html` with `Cache-Control: no-store`. It names the
/// content-hashed asset files, so a cached copy would point the browser at
/// assets a rebuilt server no longer has. nix store mtimes are a constant
/// epoch, so a weaker `no-cache` would let the browser 304-revalidate the stale
/// copy; `no-store` forces a fresh fetch each load, keeping the asset
/// references in sync with what the server actually serves.
async fn serve_index(State(state): State<AppState>) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        state.index_html.clone(),
    )
}

/// One-shot snapshot of the current monitor state as JSON, for scripts and
/// agents that want to `curl | jaq` the build tree instead of opening a
/// WebSocket. Same payload the live stream seeds each client with.
async fn state_snapshot(State(state): State<AppState>) -> Json<MonitorSnapshot> {
    Json(state.monitor.read().await.snapshot())
}

/// Upgrade the request to a WebSocket and stream deltas to it. The page is
/// served over plain HTTP on this same origin, so the browser opens `ws://`
/// with no TLS and no certificate handshake: that simplicity is the whole
/// reason for WebSocket over WebTransport here.
async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| serve_socket(socket, state.monitor, state.deltas))
}

/// Push deltas to one client on its WebSocket: a `Reset` seed first, then the
/// live feed, until the client disconnects or the broadcast closes.
#[allow(clippy::significant_drop_tightening)] // read lock must outlive subscribe(); see below.
async fn serve_socket(
    mut socket: WebSocket,
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
) {
    // Seed and subscribe under the read lock. Broadcasters drain and send while
    // holding the write lock, so nothing can be broadcast between this snapshot
    // and the subscription: the seed reflects everything applied so far, and the
    // receiver captures everything applied after, with no gap or duplicate. The
    // guard must outlive `subscribe()`, so the function opts out of clippy's
    // drop-tightening, which would otherwise reopen that gap.
    let (seed, mut receiver) = {
        let state = monitor.read().await;
        let receiver = deltas.subscribe();
        let seed = match encode(&Delta::Reset {
            snapshot: state.snapshot(),
        }) {
            Ok(seed) => seed,
            Err(error) => {
                eprintln!("nix-web-monitor: encoding WebSocket seed failed: {error:#}");
                return;
            }
        };
        (seed, receiver)
    };
    if !send_frame(&mut socket, seed).await {
        return;
    }

    loop {
        tokio::select! {
            // Detect a client close promptly even while the build is quiet, so
            // the task drops instead of lingering until the next delta send.
            incoming = socket.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {} // ignore any other client-sent frame
            },
            received = receiver.recv() => match received {
                Ok(payload) => {
                    if !send_frame(&mut socket, payload).await {
                        break;
                    }
                }
                // The client outran the ring. Reseed from a fresh snapshot and
                // take a new subscription under the read lock, so the buffered
                // backlog is dropped rather than replayed on top of the reset
                // (replay would double-apply non-idempotent deltas like log
                // appends). Same no-gap guarantee as the initial seed.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let resync = {
                        let state = monitor.read().await;
                        receiver = deltas.subscribe();
                        match encode(&Delta::Reset {
                            snapshot: state.snapshot(),
                        }) {
                            Ok(resync) => resync,
                            Err(error) => {
                                eprintln!(
                                    "nix-web-monitor: encoding WebSocket resync failed: {error:#}"
                                );
                                break;
                            }
                        }
                    };
                    if !send_frame(&mut socket, resync).await {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

/// Send one binary frame under [`SEND_TIMEOUT`]. Returns `false` when the send
/// failed or the client stopped reading past the timeout, signaling the caller
/// to drop the connection.
async fn send_frame(socket: &mut WebSocket, payload: Bytes) -> bool {
    matches!(
        timeout(SEND_TIMEOUT, socket.send(Message::Binary(payload))).await,
        Ok(Ok(()))
    )
}

/// Encode one delta as a msgpack payload. WebSocket preserves message
/// boundaries, so each delta rides exactly one binary frame with no length
/// prefix for the client to reassemble.
fn encode(delta: &Delta) -> Result<Bytes> {
    let payload = rmp_serde::to_vec_named(delta).context("serializing delta to msgpack")?;
    Ok(Bytes::from(payload))
}

async fn run_nix_command(
    nix_args: Vec<String>,
    terminal_output: TerminalOutput,
    nix_verbose: bool,
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
) -> Result<Option<i32>> {
    let mut command = Command::new("nix");
    if nix_verbose {
        command.arg("-v");
    }
    command
        .arg("--log-format")
        .arg("internal-json")
        .args(nix_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().context("spawning nix")?;
    let stdout = child.stdout.take().context("nix stdout was not captured")?;
    let stderr = child.stderr.take().context("nix stderr was not captured")?;

    // The dependency resolver runs alongside parsing: the stderr reader hands
    // it each newly-seen build derivation, it queries the edges, and drops its
    // sender when stderr ends so the resolver drains and exits.
    let (deps_tx, deps_rx) = mpsc::unbounded_channel();
    let resolver_task = tokio::spawn(resolve_dependencies(
        deps_rx,
        Arc::clone(&monitor),
        deltas.clone(),
    ));

    let stdout_task = tokio::spawn(forward_stdout(stdout, terminal_output));
    let stderr_task = tokio::spawn(parse_stderr(
        stderr,
        Arc::clone(&monitor),
        deltas.clone(),
        terminal_output,
        deps_tx,
    ));

    let status = child.wait().await.context("waiting for nix")?;
    stdout_task.await.context("joining stdout reader")??;
    stderr_task.await.context("joining stderr reader")??;
    resolver_task
        .await
        .context("joining dependency resolver")??;

    let exit_code = status.code();
    monitor.write().await.finish(exit_code);
    broadcast_deltas(&monitor, &deltas).await?;

    Ok(exit_code)
}

/// Forward Nix's stdout byte-for-byte so commands like `nix eval --raw`
/// preserve exact output (no spurious trailing newline, no UTF-8 round-trip).
/// With `--log-format internal-json`, log activity lands on stderr; stdout is
/// reserved for the command's actual output, which never needs parsing.
async fn forward_stdout<R>(stream: R, terminal_output: TerminalOutput) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut reader = stream;
    match terminal_output {
        // Still drain so the child does not block on a full pipe.
        TerminalOutput::Quiet => {
            io::copy(&mut reader, &mut io::sink())
                .await
                .context("draining nix stdout")?;
        }
        TerminalOutput::Summary | TerminalOutput::Logs => {
            io::copy(&mut reader, &mut io::stdout())
                .await
                .context("forwarding nix stdout")?;
        }
    }
    Ok(())
}

async fn parse_stderr<R>(
    stream: R,
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
    terminal_output: TerminalOutput,
    deps_tx: mpsc::UnboundedSender<String>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    // Derivations already handed to the resolver, so each is queried once.
    let mut requested: HashSet<String> = HashSet::new();
    // Read raw bytes per line, then decode lossily. `BufReader::lines()`
    // would error out on the first non-UTF-8 byte and stop draining stderr,
    // which would block the child once the pipe filled. Lossy decode keeps
    // the stream flowing for builders that emit invalid UTF-8 (binary
    // spillover, mis-set locales, etc.).
    let mut reader = BufReader::new(stream);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let read = reader
            .read_until(b'\n', &mut buf)
            .await
            .context("reading nix stderr")?;
        if read == 0 {
            break;
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        let line = String::from_utf8_lossy(&buf);
        let mut state = monitor.write().await;
        let parsed = state.apply_line(&line);
        // Collect derivations not yet queried while still holding the lock,
        // then resolve their edges off the parse path.
        let new_derivations: Vec<String> = state
            .builds
            .keys()
            .filter(|derivation| requested.insert((*derivation).clone()))
            .cloned()
            .collect();
        drop(state);
        for derivation in new_derivations {
            // Send failure means the resolver task is gone (shutdown); dropping
            // the edge request is the right behaviour then.
            let _ = deps_tx.send(derivation);
        }
        if let Some(rendered) = render_for(terminal_output, &parsed) {
            eprintln!("{rendered}");
        }
        broadcast_deltas(&monitor, &deltas).await?;
    }
    Ok(())
}

/// Drain the deltas accumulated by the latest mutation and broadcast each as an
/// encoded message. Drains and sends under the write lock so concurrent callers
/// cannot interleave frames out of the order the state machine produced them.
// Hold the write lock across the sends on purpose: draining and broadcasting
// under one lock is what keeps two concurrent callers from interleaving frames
// out of the order the state machine produced them. Clippy's drop-tightening
// would release the lock after `drain_deltas`, reopening that race.
#[allow(clippy::significant_drop_tightening)]
pub(crate) async fn broadcast_deltas(
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) -> Result<()> {
    let mut state = monitor.write().await;
    for delta in state.drain_deltas() {
        // Subscribers may all have dropped; that's fine.
        let _ = deltas.send(encode(&delta)?);
    }
    Ok(())
}

fn render_for(terminal_output: TerminalOutput, parsed: &ParsedLine) -> Option<String> {
    // Quiet truly means quiet: skip the default plain/parse-error fallthrough.
    match terminal_output {
        TerminalOutput::Quiet => None,
        TerminalOutput::Summary => default_render(parsed).or_else(|| render_summary_event(parsed)),
        TerminalOutput::Logs => default_render(parsed).or_else(|| render_log_event(parsed)),
    }
}

/// Lines every non-quiet mode shows: plain output and parse failures.
fn default_render(parsed: &ParsedLine) -> Option<String> {
    match parsed {
        ParsedLine::Plain { text } => Some(text.clone()),
        ParsedLine::ParseError { text, error } => Some(format!(
            "nix-web-monitor: could not parse event ({error}): {text}"
        )),
        ParsedLine::Event(_) => None,
    }
}

fn render_summary_event(parsed: &ParsedLine) -> Option<String> {
    let ParsedLine::Event(NixEvent::Message(message)) = parsed else {
        return None;
    };
    // Strip ANSI before checking severity: some Nix paths emit messages
    // where the "error:" / "warning:" prefix is split by ANSI sequences,
    // and a naive substring check then misses what the operator most
    // wants to see in summary mode.
    let stripped = strip_ansi(&message.message);
    if !is_operator_message(&stripped) {
        return None;
    }
    Some(stripped)
}

fn render_log_event(parsed: &ParsedLine) -> Option<String> {
    let ParsedLine::Event(event) = parsed else {
        return None;
    };
    match event {
        NixEvent::Message(message) => Some(message.message.clone()),
        NixEvent::Start(start) if !start.text.is_empty() => Some(start.text.clone()),
        NixEvent::Result(result) => match &result.result {
            nix_web_monitor_parser::ActivityResult::BuildLogLine { line }
            | nix_web_monitor_parser::ActivityResult::PostBuildLogLine { line } => {
                Some(line.clone())
            }
            nix_web_monitor_parser::ActivityResult::SetPhase { phase } => {
                Some(format!("phase: {phase}"))
            }
            nix_web_monitor_parser::ActivityResult::FetchStatus { status } => Some(status.clone()),
            nix_web_monitor_parser::ActivityResult::FileLinked { .. }
            | nix_web_monitor_parser::ActivityResult::Progress { .. }
            | nix_web_monitor_parser::ActivityResult::SetExpected { .. }
            | nix_web_monitor_parser::ActivityResult::Other { .. } => None,
        },
        NixEvent::Start(_) | NixEvent::Stop(_) | NixEvent::Unknown { .. } => None,
    }
}

fn is_operator_message(message: &str) -> bool {
    message.contains("error:") || message.contains("warning:")
}

fn validate_site_dir(site_dir: &Path) -> Result<()> {
    let index = site_dir.join("index.html");
    if !index.is_file() {
        bail!(
            "site dir {} does not contain index.html",
            site_dir.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let (deltas, _) = broadcast::channel::<Bytes>(DELTA_CHANNEL_CAPACITY);
        AppState {
            monitor: Arc::new(RwLock::new(MonitorState::default())),
            deltas,
            index_html: Bytes::from_static(b"<!doctype html><title>test</title>"),
        }
    }

    /// `/api/state` must be a wired route that serializes the live snapshot to
    /// JSON. Exercises route registration, the handler, and serde output in one
    /// shot without binding a socket or shipping a real site dir.
    #[tokio::test]
    async fn state_endpoint_serves_snapshot_json() {
        let response = router(Path::new("/nonexistent-site"), test_state())
            .oneshot(
                Request::builder()
                    .uri("/api/state")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");

        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body collects");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("body is JSON");
        assert!(json.get("builds").is_some(), "snapshot exposes builds");
        assert!(
            json.get("activities").is_some(),
            "snapshot exposes activities"
        );
    }

    /// `/ws` must be a wired route that performs the WebSocket upgrade: a plain
    /// GET without the upgrade headers is rejected, proving the handler expects
    /// a real WebSocket handshake rather than serving the static fallback.
    #[tokio::test]
    async fn ws_route_requires_websocket_upgrade() {
        let response = router(Path::new("/nonexistent-site"), test_state())
            .oneshot(
                Request::builder()
                    .uri("/ws")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");

        assert_eq!(
            response.status(),
            StatusCode::UPGRADE_REQUIRED,
            "a non-upgrade GET to /ws is rejected by the WebSocket extractor"
        );
    }

    /// `/` must serve `index.html` with `Cache-Control: no-store`. Without it,
    /// the browser caches a stale `index.html` whose asset hashes a rebuilt
    /// server no longer has, producing the wrong-MIME load failure this header
    /// exists to prevent.
    #[tokio::test]
    async fn index_is_served_no_store() {
        let response = router(Path::new("/nonexistent-site"), test_state())
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store"),
            "index.html must not be cached so asset references stay current"
        );
    }

    /// A missing asset must 404, not fall back to `index.html`. Returning HTML
    /// for a missing `/assets/*.js` is exactly what the browser rejects for the
    /// wrong MIME type once a rebuild changes the asset hashes.
    #[tokio::test]
    async fn missing_asset_404s_instead_of_html_fallback() {
        let response = router(Path::new("/nonexistent-site"), test_state())
            .oneshot(
                Request::builder()
                    .uri("/assets/index-deadbeef.js")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");

        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "a missing asset must 404 rather than serve index.html"
        );
    }

    /// An encoded delta must decode back to an equal value: this is the exact
    /// wire contract the browser decodes per WebSocket frame, so a
    /// serialization change that breaks it must fail here.
    #[test]
    fn delta_encode_round_trips_through_msgpack() {
        let delta = Delta::ExpectedSet {
            name: "build".to_owned(),
            value: 12,
        };
        let encoded = encode(&delta).expect("delta encodes");

        let decoded: Delta = rmp_serde::from_slice(&encoded).expect("payload decodes");
        assert_eq!(decoded, delta);
    }
}
