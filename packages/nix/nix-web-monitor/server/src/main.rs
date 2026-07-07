use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum};
use ignore::WalkBuilder;
use nix_web_monitor_parser::{
    Delta, MonitorSnapshot, MonitorState, NixEvent, ParsedLine, copy_to_store_source, strip_ansi,
};
use tokio::io::{self, AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::time::timeout;
use tower_http::services::ServeDir;

mod daemon;
mod dependencies;
mod emit;
mod global;
mod reasons;
use daemon::run_daemon_probe;
use dependencies::resolve_dependencies;
use global::run_global_probe;

/// Bound on the delta broadcast ring. A client that falls this far behind gets
/// resynced with a fresh `Reset` rather than the dropped frames.
const DELTA_CHANNEL_CAPACITY: usize = 1024;

/// Cap on a single WebSocket frame send. A client that completes the handshake
/// but then stops reading would otherwise park the per-client task in `send`
/// indefinitely; this bounds that to a drop instead of a permanent pin.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Parser)]
#[command(about = "Run a Nix command with quiet terminal output and a live browser monitor.")]
struct Args {
    /// Interface used by the web monitor. Defaults to all interfaces so the UI
    /// is reachable over LAN/Tailscale without a flag. The live feed is a plain
    /// WebSocket, so off-host access needs no certificate.
    //
    // `global = true` so the flag is accepted after a named subcommand too
    // (`nwm home switch --port 8080`), not just before it.
    #[arg(long, default_value = "0.0.0.0", global = true)]
    host: String,

    /// TCP port for the UI, the JSON snapshot, and the WebSocket delta feed.
    #[arg(long, default_value_t = 7532, global = true)]
    port: u16,

    /// Static UI directory. The Nix package wrapper fills this in. Always comes
    /// from the env the wrapper sets, so it is never passed after a subcommand
    /// and need not be `global` (which clap forbids on a required argument).
    /// Optional so the headless `--emit` path (which serves no UI) runs without
    /// it; the web-UI path validates it is present in [`validate_site_dir`].
    #[arg(long, env = "NIX_WEB_MONITOR_SITE_DIR")]
    site_dir: Option<PathBuf>,

    /// Exit when the Nix command finishes instead of keeping the web UI alive.
    #[arg(long, global = true)]
    exit_when_done: bool,

    /// Terminal output policy. The browser always receives the parsed stream.
    #[arg(long, value_enum, default_value_t = TerminalOutput::Summary, global = true)]
    terminal_output: TerminalOutput,

    /// Pass `-v` to Nix for richer internal-json activity events.
    #[arg(long, global = true)]
    nix_verbose: bool,

    /// Run headless: emit the build tree as NDJSON on stdout instead of serving
    /// the web UI. One compact JSON `BuildView` per line, throttled, for a
    /// programmatic consumer (the kernel `nix` module's live pane). Only the
    /// passthrough `nix …` subcommand is supported; a switch has no headless
    /// form. Exits with nix's status.
    #[arg(long, value_enum, global = true)]
    emit: Option<EmitFormat>,

    #[command(subcommand)]
    command: NwmCommand,
}

/// Machine-readable output format for the headless emitter (`--emit`).
#[derive(Clone, Copy, Debug, ValueEnum)]
enum EmitFormat {
    /// One JSON `BuildView` object per line (newline-delimited JSON).
    Ndjson,
}

/// What `nwm` runs under the monitor. The named subcommands wrap the
/// build-then-activate dance of a system switch; the external fallback keeps the
/// original behaviour of passing any other arguments straight to `nix`, so
/// `nwm build .#x` and `nwm run .#ix -- new` are unchanged.
#[derive(clap::Subcommand)]
enum NwmCommand {
    /// Build (and by default activate) a home-manager configuration, mirroring
    /// `home-manager switch` with the build and activation shown live.
    Home(SwitchSpec),

    /// Build (and by default activate) a nix-darwin system configuration,
    /// mirroring `darwin-rebuild switch`. Activation runs under `sudo`.
    Os(SwitchSpec),

    /// Serve the web UI and the machine-wide panels (the nix-daemon probe and
    /// the machine-builds view) without wrapping any Nix command, until
    /// interrupted. For running the monitor as a long-lived service
    /// (launchd/systemd). The flags that shape a wrapped command
    /// (`--exit-when-done`, `--terminal-output`, `--nix-verbose`, `--emit`)
    /// do not apply.
    Serve,

    /// Any other arguments are passed straight to `nix` (e.g. `build .#hello`,
    /// `run .#ix -- new`, or `flake update index --flake .`).
    #[command(external_subcommand)]
    Nix(Vec<String>),
}

#[derive(clap::Args)]
struct SwitchSpec {
    #[command(subcommand)]
    action: SwitchAction,
}

#[derive(clap::Subcommand)]
enum SwitchAction {
    /// Build the configuration and activate it.
    Switch(SwitchArgs),
    /// Build the configuration only, without activating.
    Build(SwitchArgs),
}

#[derive(clap::Args)]
struct SwitchArgs {
    /// Flake to build, defaulting to the current directory. Accepts a directory
    /// (`~/.config/nix`) or `dir#name` to override the configuration name (which
    /// otherwise defaults to `<user>@<host>` for home and `<host>` for os).
    flake: Option<String>,

    /// Update flake inputs before building.
    #[arg(short = 'u', long)]
    update: bool,
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
    // Inject the runtime version onto the derived command, then parse. `--help`
    // / `--version` exit inside `get_matches`; a parse error exits via
    // `Error::exit`, matching `Args::parse()`'s behavior. The version carries
    // the shared build stamp (revision, commit date, and how long ago) the Nix
    // wrapper sets in the environment; see the `build-version` crate.
    let matches = Args::command()
        .version(build_version::version_static(env!("CARGO_PKG_VERSION")))
        .get_matches();
    let args = Args::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());

    // Headless emitter: no UI, no daemon probe -- spawn nix, stream the build
    // tree as NDJSON, exit with nix's status. Only the passthrough `nix …`
    // subcommand has a headless form; a switch or a bare `serve` is a UI-only
    // mode.
    if args.emit.is_some() {
        let NwmCommand::Nix(nix_args) = args.command else {
            bail!("--emit supports only the passthrough `nix …` command");
        };
        let exit_code = emit::run(nix_args, args.nix_verbose).await?;
        std::process::exit(exit_code.unwrap_or(1));
    }

    let site_dir = args
        .site_dir
        .context("--site-dir (or NIX_WEB_MONITOR_SITE_DIR) is required to serve the web UI")?;
    validate_site_dir(&site_dir)?;

    // Serve mode: no wrapped command at all. The monitor state starts (and
    // stays) empty -- its empty command label is what tells the UI to show the
    // no-wrapped-command placeholder instead of a build tree -- while the
    // machine-wide probes feed the daemon and machine-builds panels. There is
    // no command whose exit could end the run, so it serves until interrupted.
    if matches!(args.command, NwmCommand::Serve) {
        let monitor = Arc::new(RwLock::new(MonitorState::default()));
        let ui = start_ui(&args.host, args.port, site_dir, monitor).await?;
        eprintln!("nix-web-monitor: serving the machine view (no wrapped command); Ctrl-C to stop");
        tokio::signal::ctrl_c()
            .await
            .context("waiting for Ctrl-C")?;
        ui.abort();
        return Ok(());
    }

    // Record startup before any build runs: the "what changed" reason baseline
    // uses it to exclude outputs registered during this run (see `reasons`).
    reasons::record_start_time();

    let job = build_job(args.command).await.context("planning job")?;
    let monitor = Arc::new(RwLock::new(MonitorState::new(job.command_label.clone())));
    let ui = start_ui(&args.host, args.port, site_dir, monitor).await?;

    let build = tokio::spawn(run_job(
        job,
        args.terminal_output,
        args.nix_verbose,
        Arc::clone(&ui.monitor),
        ui.deltas.clone(),
    ));

    let exit_code = build.await.context("joining job task")??;

    if !args.exit_when_done {
        eprintln!(
            "nix-web-monitor: Nix command finished with {}; press Ctrl-C to stop the web UI",
            exit_code.map_or_else(|| "no exit code".to_owned(), |code| code.to_string())
        );
        tokio::signal::ctrl_c()
            .await
            .context("waiting for Ctrl-C")?;
    }
    ui.abort();
    // Propagate Nix's exit status either way; otherwise the wrapper masks
    // build failures from shells and CI.
    std::process::exit(exit_code.unwrap_or(1));
}

/// The long-lived machinery behind the web UI, shared by every UI mode: the
/// monitor state and delta broadcast the HTTP handlers read, plus the server
/// and machine-probe tasks, all aborted together at shutdown.
struct Ui {
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
    http_server: tokio::task::JoinHandle<()>,
    daemon_probe: tokio::task::JoinHandle<()>,
    global_probe: tokio::task::JoinHandle<()>,
}

impl Ui {
    /// Stop the server and both probes. They are best-effort background tasks
    /// with no state to flush, so aborting is a clean shutdown.
    fn abort(&self) {
        self.daemon_probe.abort();
        self.global_probe.abort();
        self.http_server.abort();
    }
}

/// Bind the web server on `host:port` and start the machine-wide probes: the
/// UI, the `/api/state` snapshot, and the `/ws` delta feed on one port, plus
/// the two best-effort overlays that live for the whole life of the UI --
/// the nix-daemon syscall tracer for the daemon panel, and the machine-builds
/// poller (`nix store builds --json`, patched nix only, self-hides on stock
/// nix) for the machine panel.
async fn start_ui(
    host: &str,
    port: u16,
    site_dir: PathBuf,
    monitor: Arc<RwLock<MonitorState>>,
) -> Result<Ui> {
    let index_html =
        Bytes::from(std::fs::read(site_dir.join("index.html")).context("reading index.html")?);
    let (deltas, _) = broadcast::channel::<Bytes>(DELTA_CHANNEL_CAPACITY);

    let http_addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("invalid HTTP address {host}:{port}"))?;

    let state = AppState {
        monitor: Arc::clone(&monitor),
        deltas: deltas.clone(),
        index_html,
    };
    let http_server = serve(http_addr, site_dir, state).await?;

    eprintln!("nix-web-monitor: http://{http_addr}");

    let daemon_probe = tokio::spawn(run_daemon_probe(Arc::clone(&monitor), deltas.clone()));
    let global_probe = tokio::spawn(run_global_probe(Arc::clone(&monitor), deltas.clone()));

    Ok(Ui {
        monitor,
        deltas,
        http_server,
        daemon_probe,
        global_probe,
    })
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
        .route("/api/global-log", get(global_log))
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
        state.index_html,
    )
}

/// One-shot snapshot of the current monitor state as JSON, for scripts and
/// agents that want to `curl | jq` the build tree instead of opening a
/// WebSocket. Same payload the live stream seeds each client with.
async fn state_snapshot(State(state): State<AppState>) -> Json<MonitorSnapshot> {
    Json(state.monitor.read().await.snapshot())
}

/// Which machine build's log `/api/global-log` should tail, by derivation path.
#[derive(serde::Deserialize)]
struct GlobalLogQuery {
    drv: String,
}

/// Tail of one machine build's on-disk log, as plain text.
///
/// The derivation must be an *active* build in the machine-wide view with a
/// recorded log file: the server only opens paths the status directory itself
/// advertised, never a caller-supplied filesystem path. 404s cover both "not an
/// active build" (it finished between poll and click) and "log not written yet"
/// (the builder has produced no output), so the panel can show a quiet
/// placeholder instead of an error.
async fn global_log(
    State(state): State<AppState>,
    Query(query): Query<GlobalLogQuery>,
) -> Response {
    let Some(log_file) = global::log_file_for(&state.monitor, &query.drv).await else {
        return (
            StatusCode::NOT_FOUND,
            "not an active machine build with a recorded log",
        )
            .into_response();
    };
    match global::read_log_tail(log_file).await {
        Ok(text) => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            text,
        )
            .into_response(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (StatusCode::NOT_FOUND, "log not written yet").into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("reading build log failed: {error}"),
        )
            .into_response(),
    }
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
            snapshot: Box::new(state.snapshot()),
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
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {} // ignore any other client-sent frame
            },
            delivered = receiver.recv() => match delivered {
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
                            snapshot: Box::new(state.snapshot()),
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

/// Result of one monitored `nix` invocation.
struct NixBuildOutcome {
    exit_code: Option<i32>,
    /// Captured stdout, empty unless the call requested capture.
    stdout: String,
}

/// Run one monitored `nix` invocation: spawn it with `--log-format internal-json`,
/// parse its stderr into the build tree, and either forward or capture its
/// stdout. Returns the exit code and the captured stdout (empty unless
/// `capture_stdout`). Does **not** call [`MonitorState::finish`]: a switch runs
/// several phases under one monitor, so the caller settles the run when the whole
/// job is done.
async fn run_nix_build(
    nix_args: Vec<String>,
    terminal_output: TerminalOutput,
    nix_verbose: bool,
    capture_stdout: bool,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) -> Result<NixBuildOutcome> {
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
        Arc::clone(monitor),
        deltas.clone(),
    ));

    let stdout_task = tokio::spawn(forward_stdout(stdout, terminal_output, capture_stdout));
    let stderr_task = tokio::spawn(parse_stderr(
        stderr,
        Arc::clone(monitor),
        deltas.clone(),
        terminal_output,
        deps_tx,
    ));

    let status = child.wait().await.context("waiting for nix")?;
    let captured = stdout_task.await.context("joining stdout reader")??;
    stderr_task.await.context("joining stderr reader")??;
    resolver_task
        .await
        .context("joining dependency resolver")??;

    Ok(NixBuildOutcome {
        exit_code: status.code(),
        stdout: captured,
    })
}

/// Forward Nix's stdout byte-for-byte so commands like `nix eval --raw`
/// preserve exact output (no spurious trailing newline, no UTF-8 round-trip).
/// With `--log-format internal-json`, log activity lands on stderr; stdout is
/// reserved for the command's actual output, which never needs parsing.
///
/// When `capture` is set the stdout is collected and returned instead of
/// forwarded: a switch builds with `--print-out-paths` and needs the resulting
/// store path to activate, not to print.
async fn forward_stdout<R>(
    stream: R,
    terminal_output: TerminalOutput,
    capture: bool,
) -> Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut reader = stream;
    if capture {
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut buf)
            .await
            .context("capturing nix stdout")?;
        return Ok(String::from_utf8_lossy(&buf).into_owned());
    }
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
    Ok(String::new())
}

/// A planned unit of work and the label shown as the build-tree root.
struct Job {
    command_label: String,
    kind: JobKind,
}

enum JobKind {
    /// A single monitored `nix` invocation (passthrough or `flake update`).
    Nix { args: Vec<String> },
    /// A build-then-activate switch.
    Switch(SwitchJob),
}

#[derive(Clone, Copy)]
enum SwitchKind {
    Home,
    Os,
}

struct SwitchJob {
    kind: SwitchKind,
    /// Flake directory (or ref) the lock-update operates on.
    flake_dir: String,
    /// `<flake>#<attr>` passed to `nix build`.
    build_target: String,
    /// Activate after building (switch), or stop after the build.
    do_switch: bool,
    /// Update flake inputs before building.
    update: bool,
}

/// Resolve a parsed subcommand into a [`Job`]: the build-tree root label plus the
/// work to run. Async because switch attr resolution needs the short hostname.
async fn build_job(command: NwmCommand) -> Result<Job> {
    match command {
        // `external_subcommand` collects the nix subcommand and its args verbatim.
        NwmCommand::Nix(args) => Ok(Job {
            command_label: format!("nix {}", args.join(" ")),
            kind: JobKind::Nix { args },
        }),
        NwmCommand::Home(spec) => build_switch_job(SwitchKind::Home, spec).await,
        NwmCommand::Os(spec) => build_switch_job(SwitchKind::Os, spec).await,
        // `main` enters serve mode before job planning, so this arm is
        // unreachable; serve wraps no command and has no job to plan.
        NwmCommand::Serve => bail!("serve wraps no command, so it has no job to plan"),
    }
}

async fn build_switch_job(kind: SwitchKind, spec: SwitchSpec) -> Result<Job> {
    let (args, do_switch) = match spec.action {
        SwitchAction::Switch(args) => (args, true),
        SwitchAction::Build(args) => (args, false),
    };
    // `dir#name` overrides the configuration name; a bare value is the flake dir.
    let (flake_dir, name_override) = args.flake.map_or_else(
        || (".".to_owned(), None),
        |value| match value.split_once('#') {
            Some((dir, name)) => (dir.to_owned(), Some(name.to_owned())),
            None => (value, None),
        },
    );
    let config_name = match name_override {
        Some(name) => name,
        None => match kind {
            SwitchKind::Home => format!("{}@{}", current_user()?, short_hostname().await?),
            // Mirror `darwin-rebuild`, which defaults the flake attribute to
            // `scutil --get LocalHostName` (not `hostname -s`, which can differ).
            SwitchKind::Os => local_host_name().await?,
        },
    };
    // The attr part of the flakeref carries quotes around the config name (it
    // contains `@` / `.`); Nix's flakeref parser accepts them in a single argv.
    let attr = match kind {
        SwitchKind::Home => format!(r#"homeConfigurations."{config_name}".activationPackage"#),
        SwitchKind::Os => format!(r#"darwinConfigurations."{config_name}".system"#),
    };
    let build_target = format!("{flake_dir}#{attr}");
    Ok(Job {
        command_label: format!("nix build {build_target}"),
        kind: JobKind::Switch(SwitchJob {
            kind,
            flake_dir,
            build_target,
            do_switch,
            update: args.update,
        }),
    })
}

/// Run a planned job to completion and return its exit code.
async fn run_job(
    job: Job,
    terminal_output: TerminalOutput,
    nix_verbose: bool,
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
) -> Result<Option<i32>> {
    match job.kind {
        JobKind::Nix { args } => {
            let outcome =
                run_nix_build(args, terminal_output, nix_verbose, false, &monitor, &deltas).await?;
            settle(&monitor, &deltas, outcome.exit_code).await?;
            Ok(outcome.exit_code)
        }
        JobKind::Switch(switch) => {
            run_switch(switch, terminal_output, nix_verbose, &monitor, &deltas).await
        }
    }
}

/// Settle the monitor (`finish`) and flush the resulting deltas.
async fn settle(
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
    exit_code: Option<i32>,
) -> Result<()> {
    monitor.write().await.finish(exit_code);
    broadcast_deltas(monitor, deltas).await
}

/// Orchestrate a switch: optional flake update, the monitored build, then (for
/// `switch`) activation and an `nvd` generation diff. Each phase that bails sets
/// a human activation status so the browser shows why a later phase did not run.
async fn run_switch(
    switch: SwitchJob,
    terminal_output: TerminalOutput,
    nix_verbose: bool,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) -> Result<Option<i32>> {
    // Phase 1: update flake inputs. A switch on a half-updated lock is wrong, so
    // a failure aborts before building.
    if switch.update {
        eprintln!(
            "nix-web-monitor: updating flake inputs in {}",
            switch.flake_dir
        );
        // Run the update through the monitor so its fetches appear in the tree
        // and any failure is parsed into the error panel, not lost to the
        // terminal.
        let update_args = vec![
            "flake".to_owned(),
            "update".to_owned(),
            "--flake".to_owned(),
            switch.flake_dir.clone(),
        ];
        let update_code = run_nix_build(update_args, terminal_output, nix_verbose, false, monitor, deltas)
            .await?
            .exit_code;
        if update_code != Some(0) {
            monitor
                .write()
                .await
                .set_activation_status("skipped (flake update failed)".to_owned());
            broadcast_deltas(monitor, deltas).await?;
            settle(monitor, deltas, update_code).await?;
            return Ok(update_code.or(Some(1)));
        }
    }

    // Phase 2: build the toplevel, capturing its out path.
    let build_args = vec![
        "build".to_owned(),
        switch.build_target.clone(),
        "--no-link".to_owned(),
        "--print-out-paths".to_owned(),
    ];
    let NixBuildOutcome {
        exit_code: build_code,
        stdout,
    } = run_nix_build(build_args, terminal_output, nix_verbose, true, monitor, deltas).await?;
    if build_code != Some(0) {
        monitor
            .write()
            .await
            .set_activation_status("skipped (build failed)".to_owned());
        broadcast_deltas(monitor, deltas).await?;
        settle(monitor, deltas, build_code).await?;
        return Ok(build_code.or(Some(1)));
    }
    // The build succeeded, so it printed the activation/system store path. Both
    // targets are single-output single-installable, so there is exactly one
    // path; take the last non-empty line. A clean build that somehow printed no
    // path is a distinct failure from a failed build, and must still exit
    // non-zero rather than masquerade as success.
    let Some(out_path) = stdout
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(ToOwned::to_owned)
    else {
        monitor
            .write()
            .await
            .set_activation_status("failed (build produced no output path)".to_owned());
        broadcast_deltas(monitor, deltas).await?;
        settle(monitor, deltas, Some(1)).await?;
        return Ok(Some(1));
    };

    // The build succeeded: promote its rows to `succeeded` now, so they read as
    // done while activation runs rather than lingering as `stopped` (and sorted
    // with unfinished work) if a later activation phase fails.
    monitor.write().await.mark_builds_succeeded();
    broadcast_deltas(monitor, deltas).await?;

    if !switch.do_switch {
        // Build-only: the operator wants the store path, which the build phase
        // captured instead of forwarding (and `--no-link` left no `result`).
        println!("{out_path}");
        settle(monitor, deltas, Some(0)).await?;
        return Ok(Some(0));
    }

    // Phase 3: activate. Capture the current generation first, before the
    // activate script flips the profile symlink, so the diff compares old to new.
    let old_generation = read_old_generation(switch.kind).await;
    let activation_code =
        run_activation(&switch, &out_path, terminal_output, monitor, deltas).await?;
    if activation_code != Some(0) {
        settle(monitor, deltas, activation_code).await?;
        return Ok(activation_code.or(Some(1)));
    }

    // Phase 4: best-effort generation diff.
    if let Some(old) = old_generation {
        run_diff(&old, &out_path, monitor, deltas).await;
    }
    settle(monitor, deltas, Some(0)).await?;
    Ok(Some(0))
}

/// Run the activation script for a built configuration, streaming its output into
/// the activation subtree. home runs `<out>/activate` directly; os activation
/// needs root, so it pre-authenticates `sudo` interactively (a clean password
/// prompt on the terminal) and then runs the canonical
/// `sudo <out>/sw/bin/darwin-rebuild activate` with its output piped.
async fn run_activation(
    switch: &SwitchJob,
    out_path: &str,
    terminal_output: TerminalOutput,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) -> Result<Option<i32>> {
    let mut command;
    let display;
    let initial_step;
    match switch.kind {
        SwitchKind::Home => {
            let activate = format!("{out_path}/activate");
            command = Command::new(&activate);
            display = activate;
            // home-manager's own `Activating <step>` lines open the steps.
            initial_step = None;
        }
        SwitchKind::Os => {
            eprintln!(
                "nix-web-monitor: system activation needs sudo -- enter your password in this terminal"
            );
            let pre_auth = Command::new("sudo")
                .arg("-v")
                .status()
                .await
                .context("pre-authenticating sudo")?;
            if pre_auth.code() != Some(0) {
                monitor
                    .write()
                    .await
                    .set_activation_status("failed (sudo authentication)".to_owned());
                broadcast_deltas(monitor, deltas).await?;
                return Ok(pre_auth.code().or(Some(1)));
            }
            // Record the new generation in the system profile before activating.
            // `darwin-rebuild activate` (unlike `switch`) does NOT advance the
            // profile, so without this a switch activates without registering the
            // generation and rollbacks/diffs/reboots still see the old one. This
            // mirrors `darwin-rebuild switch` (`nix-env -p <profile> --set`).
            let set_profile = Command::new("sudo")
                .args([
                    "nix-env",
                    "-p",
                    "/nix/var/nix/profiles/system",
                    "--set",
                    out_path,
                ])
                .status()
                .await
                .context("recording system generation")?;
            if set_profile.code() != Some(0) {
                monitor
                    .write()
                    .await
                    .set_activation_status("failed (recording system generation)".to_owned());
                broadcast_deltas(monitor, deltas).await?;
                return Ok(set_profile.code().or(Some(1)));
            }
            let activate = format!("{out_path}/sw/bin/darwin-rebuild");
            command = Command::new("sudo");
            command.arg(&activate).arg("activate");
            display = format!("sudo {activate} activate");
            // nix-darwin's activate is unstructured: one step holds every line.
            initial_step = Some("activate".to_owned());
        }
    }

    monitor
        .write()
        .await
        .begin_activation(display, initial_step);
    broadcast_deltas(monitor, deltas).await?;

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().context("spawning activation")?;
    let stdout = child
        .stdout
        .take()
        .context("activation stdout was not captured")?;
    let stderr = child
        .stderr
        .take()
        .context("activation stderr was not captured")?;

    let stdout_task = tokio::spawn(stream_activation(
        stdout,
        terminal_output,
        Arc::clone(monitor),
        deltas.clone(),
    ));
    let stderr_task = tokio::spawn(stream_activation(
        stderr,
        terminal_output,
        Arc::clone(monitor),
        deltas.clone(),
    ));

    let status = child.wait().await.context("waiting for activation")?;
    stdout_task.await.context("joining activation stdout")??;
    stderr_task.await.context("joining activation stderr")??;

    let exit_code = status.code();
    monitor.write().await.finish_activation(exit_code == Some(0));
    broadcast_deltas(monitor, deltas).await?;
    Ok(exit_code)
}

/// Read one activation stream line-by-line, folding each line into the activation
/// subtree and (unless terminal output is `Quiet`) echoing it to the terminal so
/// progress shows in both places. Lossy UTF-8 decode keeps the stream flowing
/// past non-UTF-8 bytes, matching [`parse_stderr`].
async fn stream_activation<R>(
    stream: R,
    terminal_output: TerminalOutput,
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
) -> Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stream);
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let read = reader
            .read_until(b'\n', &mut buf)
            .await
            .context("reading activation output")?;
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
        monitor.write().await.push_activation_line(&line);
        broadcast_deltas(&monitor, &deltas).await?;
        // `Quiet` prints only wrapper status lines, so suppress the per-line
        // activation echo; the browser still receives every line.
        if !matches!(terminal_output, TerminalOutput::Quiet) {
            eprintln!("{line}");
        }
    }
    Ok(())
}

/// Run `nvd diff <old> <new>` and publish its output to the diff panel.
/// Best-effort: a missing `nvd` or a diff failure leaves the panel empty with a
/// terminal note rather than failing the switch, which has already succeeded.
async fn run_diff(
    old_generation: &str,
    new_generation: &str,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) {
    let output = Command::new("nvd")
        .arg("diff")
        .arg(old_generation)
        .arg(new_generation)
        .output()
        .await;
    match output {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            if !text.trim().is_empty() {
                monitor.write().await.set_diff(text);
                if let Err(error) = broadcast_deltas(monitor, deltas).await {
                    eprintln!("nix-web-monitor: broadcasting diff failed: {error:#}");
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("nix-web-monitor: nvd not found; skipping generation diff");
        }
        Err(error) => {
            eprintln!("nix-web-monitor: running nvd diff failed: {error:#}");
        }
    }
}

/// Resolve the current generation profile path so the post-switch diff has a
/// baseline. Returns `None` (diff skipped) if no candidate resolves.
async fn read_old_generation(kind: SwitchKind) -> Option<String> {
    let candidates = match kind {
        SwitchKind::Home => {
            let home = std::env::var("HOME").unwrap_or_default();
            let user = std::env::var("USER").unwrap_or_default();
            vec![
                format!("{home}/.local/state/nix/profiles/home-manager"),
                format!("/nix/var/nix/profiles/per-user/{user}/home-manager"),
            ]
        }
        SwitchKind::Os => vec!["/nix/var/nix/profiles/system".to_owned()],
    };
    for candidate in candidates {
        if let Ok(resolved) = tokio::fs::canonicalize(&candidate).await {
            return Some(resolved.to_string_lossy().into_owned());
        }
    }
    None
}

fn current_user() -> Result<String> {
    std::env::var("USER").context("USER environment variable is not set")
}

/// The short (no-domain) hostname, used to default the configuration name.
async fn short_hostname() -> Result<String> {
    let output = Command::new("hostname")
        .arg("-s")
        .output()
        .await
        .context("running hostname -s")?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if name.is_empty() {
        bail!("hostname -s returned an empty name");
    }
    Ok(name)
}

/// The macOS `LocalHostName`, which `darwin-rebuild` uses to default the flake
/// attribute. Can differ from `hostname -s`, so the os switch must use the same
/// source or it would build a different `darwinConfigurations.<name>`.
async fn local_host_name() -> Result<String> {
    let output = Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .await
        .context("running scutil --get LocalHostName")?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if name.is_empty() {
        bail!("scutil --get LocalHostName returned an empty name");
    }
    Ok(name)
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
        // A "copying <path> to the store" activity is Nix's local source copy,
        // which it reports as an unstructured activity with no byte progress.
        // Measure the source off the parse path so the row can show its size.
        if let ParsedLine::Event(NixEvent::Start(start)) = &parsed
            && let Some(source) = copy_to_store_source(&start.text)
        {
            tokio::spawn(measure_copy_size(
                start.id,
                PathBuf::from(source),
                Arc::clone(&monitor),
                deltas.clone(),
            ));
        }
        broadcast_deltas(&monitor, &deltas).await?;
    }
    Ok(())
}

/// Measure the source path of a "copying … to the store" activity and attach the
/// size to its row, so a copy Nix reports without byte progress still shows how
/// large it is. The filesystem walk runs on a blocking thread and the row is
/// re-broadcast once it lands. A measurement failure leaves the row unannotated
/// rather than failing the build.
async fn measure_copy_size(
    activity_id: u64,
    source: PathBuf,
    monitor: Arc<RwLock<MonitorState>>,
    deltas: broadcast::Sender<Bytes>,
) -> Result<()> {
    // A copy whose source is not on the local filesystem is nothing to measure.
    // `copy_to_store_source` already rejects Nix's virtual `«…»` accessor paths,
    // but an absolute path can still be a transient that has gone away by the
    // time this runs; skip it silently rather than logging a walk error.
    if !tokio::fs::try_exists(&source).await.unwrap_or(false) {
        return Ok(());
    }
    let size = match tokio::task::spawn_blocking(move || copied_size(&source)).await {
        Ok(Ok(size)) => size,
        Ok(Err(error)) => {
            eprintln!("nix-web-monitor: measuring copy source failed: {error:#}");
            return Ok(());
        }
        Err(error) => {
            eprintln!("nix-web-monitor: copy-size measurement task panicked: {error}");
            return Ok(());
        }
    };
    monitor.write().await.set_activity_size(activity_id, size);
    broadcast_deltas(&monitor, &deltas).await
}

/// Sum the apparent byte size of the files Nix would copy from `source` into the
/// store: every regular file the gitignore rules do not exclude, which mirrors
/// how a flake's source tree is assembled. `.git` is skipped because Nix never
/// copies it. The figure is an approximate hint (apparent file sizes, not the
/// NAR encoding), so a file that vanishes or is unreadable mid-walk contributes
/// nothing rather than aborting the measurement.
fn copied_size(source: &Path) -> Result<i64> {
    let mut total: u64 = 0;
    // `hidden(false)` keeps tracked dotfiles (`.github`, `.gitignore`); the
    // gitignore filters stay on by default; `.git` itself is never copied;
    // `parents(false)` confines the rules to the source tree's own ignore files,
    // ignoring any `.gitignore` in a directory above the flake root. This is an
    // approximate hint: it follows gitignore semantics rather than git's tracked
    // set, so it can differ from Nix's copy for untracked-but-unignored files.
    let walker = WalkBuilder::new(source)
        .hidden(false)
        .parents(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walker {
        // Skip an entry that vanished or is unreadable mid-walk rather than
        // aborting the whole measurement; the figure is an approximate hint, so
        // a few missing files just make it a slight undercount.
        let Ok(entry) = entry else { continue };
        if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
            && let Ok(metadata) = entry.metadata()
        {
            total = total.saturating_add(metadata.len());
        }
    }
    // The wire size is i64 (matching the progress counters); a source tree above
    // 8 EiB cannot occur, so surface the impossible overflow rather than clamp.
    i64::try_from(total).context("source tree size exceeds i64 range")
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

    /// `serve` must parse as the dedicated no-command subcommand -- not fall
    /// through to the external passthrough, which would exec a nonexistent
    /// `nix serve` -- and the shared network flags must still apply to it.
    #[test]
    fn serve_parses_as_dedicated_subcommand() {
        let args = Args::try_parse_from(["nwm", "serve", "--host", "127.0.0.1", "--port", "8080"])
            .expect("serve parses");
        assert!(matches!(args.command, NwmCommand::Serve));
        assert_eq!(args.host, "127.0.0.1");
        assert_eq!(args.port, 8080);
    }

    /// Adding the `serve` subcommand must not narrow the passthrough: any other
    /// leading word still reaches `nix` verbatim.
    #[test]
    fn other_subcommands_still_pass_through_to_nix() {
        let args = Args::try_parse_from(["nwm", "build", ".#hello"]).expect("passthrough parses");
        let NwmCommand::Nix(nix_args) = args.command else {
            panic!("expected the external passthrough");
        };
        assert_eq!(nix_args, ["build", ".#hello"]);
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

    /// `/api/global-log` serves the tail of an active machine build's log and
    /// refuses anything the machine-wide view does not currently list. This is
    /// the whole HTTP contract the panel's log drawer depends on: route, state
    /// lookup, file read, and the not-found shapes.
    #[tokio::test]
    async fn global_log_serves_active_build_logs_only() {
        let dir = std::env::temp_dir().join(format!("nwm-global-route-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let log_path = dir.join("fixture.drv");
        std::fs::write(&log_path, b"builder says hi\n").expect("write fixture log");

        let state = test_state();
        state.monitor.write().await.set_global(
            nix_web_monitor_parser::GlobalBuilds {
                detected: true,
                builds: vec![nix_web_monitor_parser::GlobalBuild {
                    drv_path: Some("/nix/store/aaa-foo.drv".to_owned()),
                    log_file: Some(log_path.to_string_lossy().into_owned()),
                    ..nix_web_monitor_parser::GlobalBuild::default()
                }],
                status: "1 active".to_owned(),
            },
        );
        let app = router(Path::new("/nonexistent-site"), state);

        let found = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/global-log?drv=%2Fnix%2Fstore%2Faaa-foo.drv")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(found.status(), StatusCode::OK);
        let body = axum::body::to_bytes(found.into_body(), 1 << 20)
            .await
            .expect("body collects");
        assert_eq!(&body[..], b"builder says hi\n");

        let unknown = app
            .oneshot(
                Request::builder()
                    .uri("/api/global-log?drv=%2Fetc%2Fpasswd")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("router responds");
        assert_eq!(
            unknown.status(),
            StatusCode::NOT_FOUND,
            "a drv the machine view does not list must not resolve to a file"
        );

        std::fs::remove_dir_all(&dir).expect("clean scratch dir");
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
            StatusCode::BAD_REQUEST,
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

    /// The activation and diff deltas ride the same wire as every other delta, so
    /// a nested `Activation` subtree must round-trip through msgpack intact: this
    /// is the contract the browser decodes per WebSocket frame during a switch.
    #[test]
    fn activation_and_diff_deltas_round_trip_through_msgpack() {
        let mut activation = nix_web_monitor_parser::Activation::default();
        activation.begin("/nix/store/x/activate".to_owned(), None, 1);
        activation.ingest_line("Activating linkGeneration", 2);
        let activation_delta = Delta::ActivationSet { activation };
        let decoded: Delta = rmp_serde::from_slice(&encode(&activation_delta).expect("encodes"))
            .expect("payload decodes");
        assert_eq!(decoded, activation_delta);

        let diff_delta = Delta::DiffSet {
            diff: "<<< old\n>>> new".to_owned(),
        };
        let decoded: Delta =
            rmp_serde::from_slice(&encode(&diff_delta).expect("encodes")).expect("payload decodes");
        assert_eq!(decoded, diff_delta);
    }
}
