use std::collections::HashSet;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::routing::get;
use clap::{Parser, ValueEnum};
use futures::stream;
use futures::{Stream, StreamExt};
use nix_web_monitor_parser::{MonitorState, NixEvent, ParsedLine, strip_ansi};
use tokio::io::{self, AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio_stream::wrappers::BroadcastStream;
use tower_http::services::{ServeDir, ServeFile};

mod dependencies;
use dependencies::resolve_dependencies;

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

    /// Port used by the web monitor.
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

#[derive(Clone)]
struct AppState {
    monitor: Arc<RwLock<MonitorState>>,
    snapshots: broadcast::Sender<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    validate_site_dir(&args.site_dir)?;

    let monitor = Arc::new(RwLock::new(MonitorState::default()));
    let (snapshots, _) = broadcast::channel(256);

    let state = AppState {
        monitor: Arc::clone(&monitor),
        snapshots: snapshots.clone(),
    };
    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .with_context(|| format!("invalid web monitor address {}:{}", args.host, args.port))?;
    let server = serve(addr, args.site_dir, state).await?;

    eprintln!("nix-web-monitor: http://{addr}");

    let build = tokio::spawn(run_nix_command(
        args.nix_args,
        args.terminal_output,
        args.nix_verbose,
        monitor,
        snapshots,
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
    server.abort();
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
    let index = site_dir.join("index.html");
    let static_files = ServeDir::new(&site_dir).fallback(ServeFile::new(index));
    let app = Router::new()
        .route("/api/events", get(events))
        .fallback_service(static_files)
        .with_state(state);

    Ok(tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("nix-web-monitor: web server failed: {error}");
        }
    }))
}

async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // Subscribe before sending the seed so any snapshot produced between
    // serialising and forwarding still reaches this client.
    let receiver = state.snapshots.subscribe();
    let seed = serde_json::to_string(&state.monitor.read().await.snapshot())
        .ok()
        .map(|payload| Event::default().event("snapshot").data(payload));

    let seed_stream = stream::iter(seed.map(Ok));
    let live_stream = BroadcastStream::new(receiver).filter_map(|message| async move {
        match message {
            Ok(data) => Some(Ok(Event::default().event("snapshot").data(data))),
            Err(broadcast_error) => Some(Ok(Event::default()
                .event("monitor-error")
                .data(broadcast_error.to_string()))),
        }
    });

    Sse::new(seed_stream.chain(live_stream))
}

async fn run_nix_command(
    nix_args: Vec<String>,
    terminal_output: TerminalOutput,
    nix_verbose: bool,
    monitor: Arc<RwLock<MonitorState>>,
    snapshots: broadcast::Sender<String>,
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
        snapshots.clone(),
    ));

    let stdout_task = tokio::spawn(forward_stdout(stdout, terminal_output));
    let stderr_task = tokio::spawn(parse_stderr(
        stderr,
        Arc::clone(&monitor),
        snapshots.clone(),
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
    publish_snapshot(&monitor, &snapshots).await?;

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
    snapshots: broadcast::Sender<String>,
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
        publish_snapshot(&monitor, &snapshots).await?;
    }
    Ok(())
}

pub(crate) async fn publish_snapshot(
    monitor: &Arc<RwLock<MonitorState>>,
    snapshots: &broadcast::Sender<String>,
) -> Result<()> {
    let payload = serde_json::to_string(&monitor.read().await.snapshot())
        .context("serializing monitor snapshot")?;
    // Subscribers may all have dropped; that's fine.
    let _ = snapshots.send(payload);
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
