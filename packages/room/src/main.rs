use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::Context;
use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use clap::Parser;
use futures::{SinkExt, StreamExt};
use loro::{ExportMode, LoroDoc};
use tokio::sync::{Mutex, broadcast};
use tower_http::services::{ServeDir, ServeFile};

#[derive(Parser, Debug)]
#[command(about = "Serve a multiplayer team room backed by a Loro CRDT document")]
struct Args {
    #[arg(long, env = "ROOM_HOST", default_value_t = IpAddr::V4(Ipv4Addr::UNSPECIFIED))]
    host: IpAddr,

    #[arg(long, env = "ROOM_PORT", default_value_t = 8080)]
    port: u16,

    #[arg(long, env = "ROOM_SITE_DIR")]
    site_dir: Option<PathBuf>,
}

#[derive(Clone)]
struct AppState {
    doc: Arc<Mutex<LoroDoc>>,
    updates: broadcast::Sender<Broadcast>,
}

#[derive(Clone)]
struct Broadcast {
    // Connection that produced the update. Receivers skip frames they sent
    // themselves so a client's own edit is not echoed back across the wire.
    origin: u64,
    bytes: Arc<[u8]>,
}

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let site_dir = args.site_dir.or_else(|| std::env::var_os("ROOM_SITE_DIR").map(PathBuf::from));
    let site_dir = site_dir.context("set --site-dir or ROOM_SITE_DIR to the built Svelte site")?;
    let doc = LoroDoc::new();
    // Server peer id is fixed; clients pick their own random peer id so concurrent
    // ops from different sessions do not collide.
    doc.set_peer_id(1).context("failed to set room document peer id")?;
    let (updates, _) = broadcast::channel(256);
    let state = AppState {
        doc: Arc::new(Mutex::new(doc)),
        updates,
    };
    let index = site_dir.join("index.html");
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .fallback_service(ServeDir::new(&site_dir).not_found_service(ServeFile::new(index)))
        .with_state(state);
    let addr = SocketAddr::new(args.host, args.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("room server failed")
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let (mut sender, mut receiver) = socket.split();

    let snapshot = {
        let doc = state.doc.lock().await;
        doc.export(ExportMode::Snapshot)
    };
    let Ok(snapshot) = snapshot else {
        return;
    };
    if sender.send(Message::Binary(snapshot.into())).await.is_err() {
        return;
    }

    let mut updates = state.updates.subscribe();
    let send_task = tokio::spawn(async move {
        while let Ok(broadcast) = updates.recv().await {
            if broadcast.origin == client_id {
                continue;
            }
            let payload = broadcast.bytes.as_ref().to_vec();
            if sender.send(Message::Binary(payload.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(message)) = receiver.next().await {
        let Message::Binary(bytes) = message else {
            continue;
        };
        {
            let doc = state.doc.lock().await;
            if doc.import(&bytes).is_err() {
                continue;
            }
        }
        let _ = state.updates.send(Broadcast {
            origin: client_id,
            bytes: Arc::from(bytes.as_ref()),
        });
    }

    send_task.abort();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
