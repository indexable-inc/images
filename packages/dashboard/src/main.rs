//! Standalone aggregator: one web canvas for every resource producer on the
//! machine.
//!
//! Each producing process exposes its panes over a unix socket in the discovery
//! directory ([`discovery_dir`](dashboard_core::discovery_dir)); see the producer
//! side in [`dashboard_core::Publisher`] (the `tui` crate adapts its PTY manager
//! into terminal panes, a VM controller publishes an HTML or data pane, and so
//! on). This binary scans that directory, connects to every socket, folds each
//! producer's stream into one Loro document under its own scope, and serves the
//! shared board over HTTP + SSE. No producer owns the server and exactly one
//! process binds a TCP port, so any number of producers can come and go behind
//! one stable URL.
//!
//! `dashboard demo` runs a self-contained producer that publishes one pane of
//! each kind, so the canvas can be exercised with no other process running.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand};
use dashboard_core::{
    ExecView, Hub, Pane, ProducerSnapshot, Publisher, RecordingStore, TerminalView, discovery_dir,
    serve_hub, socket_path,
};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::net::UnixStream;

/// Aggregate every ix resource producer socket into one live web canvas.
#[derive(Parser)]
#[command(name = "dashboard", version, about)]
struct Cli {
    /// Address to bind the dashboard on. `0.0.0.0` exposes it on the network.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to bind. `0` picks an ephemeral port, printed on startup.
    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// Directory of producer sockets to watch (serve) or publish into (demo).
    /// Defaults to the ix discovery directory (`$IX_DASH_DIR`,
    /// `$XDG_RUNTIME_DIR/ix-dash`, or `/tmp/ix-dash-*`). Global so it works
    /// before or after the subcommand.
    #[arg(long, global = true)]
    dir: Option<PathBuf>,

    /// How often to rescan the directory for new or removed sockets, in
    /// milliseconds.
    #[arg(long, default_value_t = 500)]
    rescan_ms: u64,

    /// How often to persist the live board as a replayable recording, in
    /// milliseconds. `0` disables on-disk recording (replay still works for the
    /// current browser session from the live stream).
    #[arg(long, default_value_t = 5000)]
    record_ms: u64,

    /// Directory recordings are written to. Defaults to the ix recordings
    /// directory (`$IX_DASH_RECORDINGS`, `$XDG_STATE_HOME/ix-dash/recordings`,
    /// or `~/.local/state/ix-dash/recordings`).
    #[arg(long)]
    record_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Publish one pane of every kind (terminal, html, data) to the discovery
    /// directory until interrupted, for exercising the canvas standalone.
    Demo,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Demo) => run_demo(cli.dir).await,
        None => run_server(&cli).await,
    }
}

/// Serve the aggregated canvas and watch the discovery directory.
async fn run_server(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let dir = cli.dir.clone().unwrap_or_else(discovery_dir);
    let addr: SocketAddr = format!("{}:{}", cli.host, cli.port).parse()?;

    let hub = Hub::new();
    let handle = tokio::runtime::Handle::current();

    // Persist the live board so a session survives a restart and can be shared.
    // A store failure (e.g. an unwritable directory) is not fatal: the dashboard
    // still serves live, and replay works from the browser's own history.
    let recordings = match recording_store(cli) {
        Ok(store) => Some(Arc::new(store)),
        Err(error) => {
            eprintln!("dashboard: recordings disabled ({error})");
            None
        }
    };

    // The process runtime outlives the dashboard, so the server and discovery
    // loop spawned on it run for the lifetime of the binary.
    let (mut dashboard, _stop_rx) =
        serve_hub(hub.clone(), addr, recordings.clone(), &handle).await?;
    println!(
        "dashboard: serving {}  (watching {})",
        dashboard.url(),
        dir.display()
    );

    // Held until shutdown so a final snapshot captures the last interval of
    // changes, which the periodic recorder task would otherwise lose when it is
    // aborted on stop.
    let recording_session = recordings
        .as_ref()
        .filter(|_| cli.record_ms > 0)
        .map(|store| {
            let (id, recorder) =
                store.spawn_recorder(hub.clone(), Duration::from_millis(cli.record_ms), &handle);
            println!("dashboard: recording to {} ({id})", store.dir().display());
            dashboard.push_task(recorder);
            (store.clone(), id)
        });

    let connected = Arc::new(Mutex::new(HashSet::new()));
    let discovery = tokio::spawn(discover(
        hub.clone(),
        dir,
        connected,
        Duration::from_millis(cli.rescan_ms),
    ));
    dashboard.push_task(discovery);

    tokio::signal::ctrl_c().await?;
    println!("\ndashboard: shutting down");
    dashboard.stop().await;
    // The periodic recorder task was aborted by `stop`; write one last snapshot
    // now that the document is final, so the recording does not lose the last
    // interval of changes before exit.
    if let Some((store, id)) = recording_session {
        let _ = store.save(&id, &hub.export_snapshot());
    }
    Ok(())
}

/// Rescan `dir` on a fixed interval and spawn a reader for each newly-seen
/// socket. `connected` is the set of sockets currently being read, so a socket is
/// read by exactly one task and a re-created socket reconnects after its reader
/// finishes.
async fn discover(
    hub: Arc<Hub>,
    dir: PathBuf,
    connected: Arc<Mutex<HashSet<PathBuf>>>,
    rescan: Duration,
) {
    loop {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("sock") {
                    continue;
                }
                if !connected.lock().expect("connected set poisoned").insert(path.clone()) {
                    continue;
                }
                let hub = hub.clone();
                let connected = connected.clone();
                tokio::spawn(async move {
                    read_producer(&hub, &path).await;
                    connected.lock().expect("connected set poisoned").remove(&path);
                });
            }
        }
        tokio::time::sleep(rescan).await;
    }
}

/// Connect to one producer socket and fold its NDJSON stream into the hub until
/// the producer hangs up. On disconnect, the producer's scope is dropped so its
/// panes leave the board; a stale socket file (connection refused) is reaped.
async fn read_producer(hub: &Hub, path: &Path) {
    let stream = match UnixStream::connect(path).await {
        Ok(stream) => stream,
        Err(error) => {
            // A bound, listening socket accepts immediately, so a refusal means
            // the socket file outlived its producer. Reap it, but only if it is
            // actually a socket: a regular `*.sock` file the user dropped in the
            // watched directory also refuses, and must not be deleted.
            if error.kind() == std::io::ErrorKind::ConnectionRefused && is_socket(path) {
                let _ = std::fs::remove_file(path);
            }
            return;
        }
    };

    let mut lines = BufReader::new(stream).lines();
    let mut producer_id: Option<String> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.is_empty() {
            continue;
        }
        // Skip a malformed line rather than dropping the producer: a future wire
        // version should degrade, not disconnect a working board.
        if let Ok(snapshot) = serde_json::from_str::<ProducerSnapshot>(&line) {
            producer_id = Some(snapshot.producer.clone());
            hub.apply_scope(&snapshot.producer, &snapshot.panes);
        }
    }

    if let Some(id) = producer_id {
        hub.remove_scope(&id);
    }
}

/// Whether `path` is a unix socket, used to avoid reaping a regular file that a
/// user happened to name `*.sock` in the watched directory.
fn is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt as _;
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_socket())
}

/// Open the recordings store at the configured directory, or the default one.
fn recording_store(cli: &Cli) -> Result<RecordingStore, Box<dyn std::error::Error>> {
    let store = match cli.record_dir.clone() {
        Some(dir) => RecordingStore::new(dir)?,
        None => RecordingStore::open_default()?,
    };
    Ok(store)
}

/// Run a demo producer: publish one pane of every kind, each ticking once a
/// second, until interrupted. Exercises the whole pipeline (publisher socket,
/// aggregator fold, every renderer) with no other process.
async fn run_demo(dir: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let path = dir.map_or_else(socket_path, |d| {
        d.join(format!("{}-demo.sock", std::process::id()))
    });
    let mut publisher = Publisher::bind(path.clone(), &tokio::runtime::Handle::current())?;
    println!(
        "dashboard demo: publishing 4 panes on {} (run `dashboard` in another shell)",
        path.display()
    );

    let mut tick: u64 = 0;
    loop {
        publisher.publish(&demo_panes(tick));
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(1)) => tick += 1,
            result = tokio::signal::ctrl_c() => {
                result?;
                break;
            }
        }
    }
    println!("\ndashboard demo: shutting down");
    publisher.stop().await;
    Ok(())
}

/// The demo's panes at a given tick: one of every kind.
fn demo_panes(tick: u64) -> Vec<Pane> {
    let bar = "#".repeat(usize::try_from((tick % 20) + 1).unwrap_or(20));
    let terminal = Pane::terminal(
        "demo-term",
        TerminalView {
            command: "demo".to_owned(),
            args: "--tick".to_owned(),
            rows: 3,
            cols: 40,
            alive: true,
            // A green "tick" line, exercising the SGR renderer.
            screen: format!("\x1b[32mtick {tick}\x1b[0m\n{bar}\nany resource is a pane"),
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: false,
            cursor_shape: "block".to_owned(),
            exit_code: None,
        },
    );
    let html = Pane::html(
        "demo-html",
        "html pane",
        format!(
            "<div style=\"font:14px ui-monospace,monospace;padding:14px;color:#89b4fa\">\
             <div style=\"font-size:28px\">{tick}</div>\
             <div style=\"opacity:.6\">a producer-rendered HTML view</div></div>"
        ),
    );
    let data = Pane::data(
        "demo-data",
        "data pane",
        "kv",
        serde_json::json!({
            "tick": tick,
            "status": if tick.is_multiple_of(2) { "even" } else { "odd" },
            "load": (f64::from(u32::try_from(tick % 100).unwrap_or(0)) / 100.0),
            "nested": {"a": 1, "b": [1, 2, 3]},
        }),
    );
    // An exec pane: alternate running and finished so the demo shows both states
    // (the running spinner and a captured stdout/stderr result).
    let running = tick.is_multiple_of(2);
    let exec = Pane::exec(
        "demo-exec",
        ExecView {
            source: format!("import subprocess\nsubprocess.run(['echo', 'tick {tick}'])"),
            lang: "python".to_owned(),
            stdout: if running { String::new() } else { format!("tick {tick}\n") },
            stderr: String::new(),
            result: String::new(),
            running,
            ok: if running { None } else { Some(true) },
        },
    );
    vec![terminal, html, exec, data]
}
