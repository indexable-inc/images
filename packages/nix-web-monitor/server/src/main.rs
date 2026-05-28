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
use axum::routing::get;
use bytes::{BufMut, Bytes, BytesMut};
use clap::{Parser, ValueEnum};
use nix_web_monitor_parser::{Delta, MonitorSnapshot, MonitorState, NixEvent, ParsedLine, strip_ansi};
use serde::Serialize;
use tokio::io::{self, AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast, mpsc};
use tower_http::services::{ServeDir, ServeFile};
use wtransport::endpoint::IncomingSession;
use wtransport::{Endpoint, Identity, ServerConfig};

mod dependencies;
use dependencies::resolve_dependencies;

/// Bound on the delta broadcast ring. A client that falls this far behind gets
/// resynced with a fresh `Reset` rather than the dropped frames.
const DELTA_CHANNEL_CAPACITY: usize = 1024;

/// WebTransport keep-alive. Localhost rarely idles, but a short interval keeps
/// the QUIC connection from being reaped during a quiet stretch of a long build.
const KEEP_ALIVE: Duration = Duration::from_secs(3);

#[derive(Parser)]
#[command(
    about = "Run a Nix command with quiet terminal output and a live browser monitor.",
    version
)]
#[allow(clippy::struct_field_names)] // `nix_args` is the wire-level name passed to `nix`; renaming would hurt the CLI help text.
struct Args {
    /// Interface used by the web monitor.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// TCP port for the static UI and JSON endpoints.
    #[arg(long, default_value_t = 7532)]
    port: u16,

    /// UDP port for the WebTransport (HTTP/3) live delta stream.
    #[arg(long, default_value_t = 7533)]
    udp_port: u16,

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

/// Shared state for the HTTP handlers. The live delta feed rides WebTransport,
/// so the broadcast sender lives with those tasks, not here; HTTP only serves
/// the one-shot snapshot and the transport handshake.
#[derive(Clone)]
struct AppState {
    monitor: Arc<RwLock<MonitorState>>,
    /// What the browser needs to dial the WebTransport endpoint.
    transport: TransportInfo,
}

/// Connection details the page fetches over plain HTTP before opening the
/// WebTransport session. The cert hash lets the browser pin our ephemeral
/// self-signed certificate via `serverCertificateHashes` instead of the public
/// PKI, which is what makes a localhost HTTP/3 server reachable without a CA.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransportInfo {
    /// UDP port of the WebTransport endpoint.
    port: u16,
    /// SHA-256 of the server's self-signed certificate (32 bytes).
    cert_hash: Vec<u8>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    validate_site_dir(&args.site_dir)?;

    // ECDSA P-256, 14-day validity: exactly what `serverCertificateHashes`
    // requires, so the browser can pin this cert without a CA.
    let identity = Identity::self_signed(["localhost", "127.0.0.1", "::1"])
        .context("generating self-signed WebTransport identity")?;
    let cert_hash = identity.certificate_chain().as_slice()[0]
        .hash()
        .as_ref()
        .to_vec();

    let monitor = Arc::new(RwLock::new(MonitorState::default()));
    let (deltas, _) = broadcast::channel::<Bytes>(DELTA_CHANNEL_CAPACITY);

    let http_addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .with_context(|| format!("invalid HTTP address {}:{}", args.host, args.port))?;
    let udp_addr: SocketAddr = format!("{}:{}", args.host, args.udp_port)
        .parse()
        .with_context(|| format!("invalid WebTransport address {}:{}", args.host, args.udp_port))?;

    let state = AppState {
        monitor: Arc::clone(&monitor),
        transport: TransportInfo {
            port: args.udp_port,
            cert_hash,
        },
    };
    let http_server = serve(http_addr, args.site_dir, state).await?;
    let wt_server = spawn_webtransport(udp_addr, identity, Arc::clone(&monitor), deltas.clone())?;

    eprintln!("nix-web-monitor: http://{http_addr} (WebTransport on udp/{})", args.udp_port);

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
    wt_server.abort();
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

/// Build the HTTP router: the JSON endpoints plus the static UI fallback. The
/// live stream rides WebTransport, not HTTP, so the only API routes here are the
/// one-shot snapshot and the transport handshake. Split from [`serve`] so it can
/// be exercised without binding a socket.
fn router(site_dir: &Path, state: AppState) -> Router {
    let index = site_dir.join("index.html");
    let static_files = ServeDir::new(site_dir).fallback(ServeFile::new(index));
    Router::new()
        .route("/api/state", get(state_snapshot))
        .route("/api/transport", get(transport_info))
        .fallback_service(static_files)
        .with_state(state)
}

/// One-shot snapshot of the current monitor state as JSON, for scripts and
/// agents that want to `curl | jq` the build tree instead of opening a
/// WebTransport session. Same payload the live stream seeds each client with.
async fn state_snapshot(State(state): State<AppState>) -> Json<MonitorSnapshot> {
    Json(state.monitor.read().await.snapshot())
}

/// WebTransport handshake details the page needs before dialing the HTTP/3
/// endpoint: the UDP port and the certificate hash to pin.
async fn transport_info(State(state): State<AppState>) -> Json<TransportInfo> {
    Json(state.transport)
}

/// Bind the WebTransport endpoint and serve one delta stream per session.
fn spawn_webtransport(
    addr: SocketAddr,
    identity: Identity,
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
) -> Result<tokio::task::JoinHandle<()>> {
    let config = ServerConfig::builder()
        .with_bind_address(addr)
        .with_identity(identity)
        .keep_alive_interval(Some(KEEP_ALIVE))
        .build();
    let endpoint =
        Endpoint::server(config).with_context(|| format!("binding WebTransport endpoint on {addr}"))?;

    Ok(tokio::spawn(async move {
        loop {
            let incoming = endpoint.accept().await;
            let monitor = Arc::clone(&monitor);
            let deltas = deltas.clone();
            tokio::spawn(async move {
                if let Err(error) = serve_session(incoming, &monitor, &deltas).await {
                    eprintln!("nix-web-monitor: WebTransport session ended: {error:#}");
                }
            });
        }
    }))
}

/// Accept one WebTransport session and push deltas on a unidirectional stream:
/// a `Reset` seed first, then the live feed.
#[allow(clippy::significant_drop_tightening)] // read lock must outlive subscribe(); see below.
async fn serve_session(
    incoming: IncomingSession,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) -> Result<()> {
    let connection = incoming
        .await
        .context("awaiting WebTransport session request")?
        .accept()
        .await
        .context("accepting WebTransport session")?;
    let mut stream = connection
        .open_uni()
        .await
        .context("opening unidirectional stream")?
        .await
        .context("establishing unidirectional stream")?;

    // Seed and subscribe under the read lock. Broadcasters drain and send while
    // holding the write lock, so nothing can be broadcast between this snapshot
    // and the subscription: the seed reflects everything applied so far, and the
    // receiver captures everything applied after, with no gap or duplicate. The
    // guard must outlive `subscribe()`, so the function opts out of clippy's
    // drop-tightening, which would otherwise reopen that gap.
    let (seed, mut receiver) = {
        let state = monitor.read().await;
        let receiver = deltas.subscribe();
        let seed = frame(&Delta::Reset {
            snapshot: state.snapshot(),
        })?;
        (seed, receiver)
    };
    stream.write_all(&seed).await.context("writing seed frame")?;

    loop {
        match receiver.recv().await {
            Ok(payload) => stream.write_all(&payload).await.context("writing delta frame")?,
            // A slow client outran the ring; resync from a fresh snapshot
            // rather than leaving it stuck on stale state.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let resync = frame(&Delta::Reset {
                    snapshot: monitor.read().await.snapshot(),
                })?;
                stream
                    .write_all(&resync)
                    .await
                    .context("writing resync frame")?;
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    Ok(())
}

/// Encode one delta as a length-prefixed msgpack frame: a `u32` big-endian byte
/// count followed by the payload, so the client can split the stream back into
/// discrete deltas.
fn frame(delta: &Delta) -> Result<Bytes> {
    let payload = rmp_serde::to_vec_named(delta).context("serializing delta to msgpack")?;
    let len = u32::try_from(payload.len()).context("delta frame exceeds u32 length")?;
    let mut buf = BytesMut::with_capacity(4 + payload.len());
    buf.put_u32(len);
    buf.put_slice(&payload);
    Ok(buf.freeze())
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

/// Drain the deltas accumulated by the latest mutation and broadcast each as a
/// framed message. Drains and sends under the write lock so concurrent callers
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
        let _ = deltas.send(frame(&delta)?);
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
        AppState {
            monitor: Arc::new(RwLock::new(MonitorState::default())),
            transport: TransportInfo {
                port: 7533,
                cert_hash: vec![0u8; 32],
            },
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

    /// The handshake route must expose the UDP port and the 32-byte cert hash
    /// the browser pins; without both the page cannot open a session.
    #[tokio::test]
    async fn transport_endpoint_exposes_port_and_cert_hash() {
        let response = router(Path::new("/nonexistent-site"), test_state())
            .oneshot(
                Request::builder()
                    .uri("/api/transport")
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
        assert_eq!(json.get("port").and_then(serde_json::Value::as_u64), Some(7533));
        assert_eq!(
            json.get("certHash").and_then(serde_json::Value::as_array).map(Vec::len),
            Some(32),
            "cert hash is the 32-byte SHA-256 the browser pins"
        );
    }

    /// A framed delta must carry its msgpack length prefix and decode back to an
    /// equal value: this is the exact wire contract the browser reframes and
    /// decodes, so a serialization change that breaks it must fail here.
    #[test]
    fn delta_frames_round_trip_through_msgpack() {
        let delta = Delta::ExpectedSet {
            name: "build".to_owned(),
            value: 12,
        };
        let framed = frame(&delta).expect("delta frames");

        let len = u32::from_be_bytes(framed[..4].try_into().expect("length prefix")) as usize;
        assert_eq!(len, framed.len() - 4, "prefix counts the payload bytes");

        let decoded: Delta = rmp_serde::from_slice(&framed[4..]).expect("payload decodes");
        assert_eq!(decoded, delta);
    }
}
