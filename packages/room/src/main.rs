use std::{
    collections::BTreeMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
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
use loro::LoroDoc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Mutex, broadcast};
use tower_http::services::{ServeDir, ServeFile};

#[derive(Parser, Debug)]
#[command(about = "Serve a multiplayer team room")]
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
    room: Arc<Mutex<RoomState>>,
    doc: Arc<Mutex<LoroDoc>>,
    events: broadcast::Sender<ServerEvent>,
}

#[derive(Default, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RoomState {
    participants: BTreeMap<String, Participant>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Participant {
    id: String,
    name: String,
    color: String,
    focus: String,
    draft: String,
    codex: CodexStatus,
    last_seen_ms: u128,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CodexStatus {
    task: String,
    status: AgentStatus,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "kebab-case")]
enum AgentStatus {
    Idle,
    Thinking,
    Editing,
    Reviewing,
    Blocked,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ClientEvent {
    Presence {
        id: String,
        name: String,
        color: String,
    },
    Focus {
        id: String,
        focus: String,
    },
    Draft {
        id: String,
        draft: String,
    },
    Codex {
        id: String,
        task: String,
        status: AgentStatus,
    },
    Leave {
        id: String,
    },
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ServerEvent {
    Snapshot { state: RoomState },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let site_dir = args.site_dir.or_else(|| std::env::var_os("ROOM_SITE_DIR").map(PathBuf::from));
    let site_dir = site_dir.context("set --site-dir or ROOM_SITE_DIR to the built Svelte site")?;
    let (events, _) = broadcast::channel(128);
    let doc = LoroDoc::new();
    doc.set_peer_id(1).context("failed to set room document peer id")?;
    let state = AppState {
        room: Arc::new(Mutex::new(RoomState::default())),
        doc: Arc::new(Mutex::new(doc)),
        events,
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
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.events.subscribe();
    send_snapshot(&state).await;

    let send_task = tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            let Ok(text) = serde_json::to_string(&event) else {
                continue;
            };
            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(message)) = receiver.next().await {
        if let Message::Text(text) = message {
            let Ok(event) = serde_json::from_str::<ClientEvent>(&text) else {
                continue;
            };
            apply_event(&state, event).await;
        }
    }

    send_task.abort();
}

async fn apply_event(state: &AppState, event: ClientEvent) {
    record_event(state, &event).await;

    let mut room = state.room.lock().await;
    match event {
        ClientEvent::Presence { id, name, color } => {
            room.participants
                .entry(id.clone())
                .and_modify(|participant| {
                    participant.name.clone_from(&name);
                    participant.color.clone_from(&color);
                    participant.last_seen_ms = now_ms();
                })
                .or_insert_with(|| Participant {
                    id,
                    name,
                    color,
                    focus: "overview".to_owned(),
                    draft: String::new(),
                    codex: CodexStatus {
                        task: "waiting for a task".to_owned(),
                        status: AgentStatus::Idle,
                    },
                    last_seen_ms: now_ms(),
                });
        }
        ClientEvent::Focus { id, focus } => {
            if let Some(participant) = room.participants.get_mut(&id) {
                participant.focus = focus;
                participant.last_seen_ms = now_ms();
            }
        }
        ClientEvent::Draft { id, draft } => {
            if let Some(participant) = room.participants.get_mut(&id) {
                participant.draft = draft;
                participant.last_seen_ms = now_ms();
            }
        }
        ClientEvent::Codex { id, task, status } => {
            if let Some(participant) = room.participants.get_mut(&id) {
                participant.codex = CodexStatus { task, status };
                participant.last_seen_ms = now_ms();
            }
        }
        ClientEvent::Leave { id } => {
            room.participants.remove(&id);
        }
    }

    let _ = state.events.send(ServerEvent::Snapshot {
        state: room.clone(),
    });
}

async fn record_event(state: &AppState, event: &ClientEvent) {
    let Ok(value) = serde_json::to_value(event) else {
        return;
    };
    let doc = state.doc.lock().await;
    let events = doc.get_list("events");
    if events
        .insert(
            events.len(),
            json!({
                "atMs": now_ms(),
                "event": value,
            }),
        )
        .is_ok()
    {
        doc.commit();
    }
}

async fn send_snapshot(state: &AppState) {
    let room = state.room.lock().await.clone();
    let _ = state.events.send(ServerEvent::Snapshot { state: room });
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
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
