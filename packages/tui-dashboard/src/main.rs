//! Standalone aggregator: one web dashboard for every `tui` producer on the
//! machine.
//!
//! Each producing process exposes its terminals over a unix socket in the
//! discovery directory ([`socket_dir`](tui_dashboard_core::socket_dir)); see the
//! producer side in `tui::publish`. This binary is started by hand, scans that
//! directory,
//! connects to every socket, folds each producer's stream into one Loro document
//! under its own scope, and serves the shared grid over HTTP + SSE. No producer
//! owns the server and exactly one process binds a TCP port, so any number of
//! agents can come and go behind one stable URL.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::net::UnixStream;
use tui_dashboard_core::{Hub, ProducerSnapshot, serve_hub, socket_dir};

/// Aggregate every ix `tui` producer socket into one live web dashboard.
#[derive(Parser)]
#[command(name = "tui-dashboard", version, about)]
struct Args {
    /// Address to bind the dashboard on. `0.0.0.0` exposes it on the network.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to bind. `0` picks an ephemeral port, printed on startup.
    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// Directory of producer sockets to watch. Defaults to the ix-tui discovery
    /// directory (`$IX_TUI_DIR`, `$XDG_RUNTIME_DIR/ix-tui`, or `/tmp/ix-tui-*`).
    #[arg(long)]
    dir: Option<PathBuf>,

    /// How often to rescan the directory for new or removed sockets, in
    /// milliseconds.
    #[arg(long, default_value_t = 500)]
    rescan_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let dir = args.dir.unwrap_or_else(socket_dir);
    let addr: SocketAddr = format!("{}:{}", args.host, args.port).parse()?;

    let hub = Hub::new();
    // The process runtime outlives the dashboard, so the server and discovery
    // loop spawned on it run for the lifetime of the binary.
    let (mut dashboard, _stop_rx) = serve_hub(hub.clone(), addr, &tokio::runtime::Handle::current()).await?;
    println!(
        "tui-dashboard: serving {}  (watching {})",
        dashboard.url(),
        dir.display()
    );

    let connected = Arc::new(Mutex::new(HashSet::new()));
    let discovery = tokio::spawn(discover(
        hub,
        dir,
        connected,
        Duration::from_millis(args.rescan_ms),
    ));
    dashboard.push_task(discovery);

    tokio::signal::ctrl_c().await?;
    println!("\ntui-dashboard: shutting down");
    dashboard.stop().await;
    Ok(())
}

/// Rescan `dir` on a fixed interval and spawn a reader for each newly-seen
/// socket. `connected` is the set of sockets currently being read, so a socket
/// is read by exactly one task and a re-created socket reconnects after its
/// reader finishes.
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
                    connected
                        .lock()
                        .expect("connected set poisoned")
                        .remove(&path);
                });
            }
        }
        tokio::time::sleep(rescan).await;
    }
}

/// Connect to one producer socket and fold its NDJSON stream into the hub until
/// the producer hangs up. On disconnect, the producer's scope is dropped so its
/// terminals leave the grid; a stale socket file (connection refused) is reaped.
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
        // Skip a malformed line rather than dropping the producer: a future
        // wire version should degrade, not disconnect a working terminal.
        if let Ok(snapshot) = serde_json::from_str::<ProducerSnapshot>(&line) {
            producer_id = Some(snapshot.producer.clone());
            hub.apply_scope(&snapshot.producer, &snapshot.terminals);
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
