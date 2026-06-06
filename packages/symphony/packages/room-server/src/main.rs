// Multiplayer team room: an Amp-style live view of every Codex thread
// running on the ix tailnet.
//
// The server is intentionally small. It exposes:
//
//   GET  /api/threads               paginated, latest-first thread index
//   GET  /api/threads/:id           single thread metadata
//   GET  /api/threads/:id/messages  paginated transcript for a thread
//   GET  /api/wt/info               WebTransport URL + cert hash
//   QUIC https://host:wt_port       WebTransport: sync streams + audio datagrams
//   GET  /                          built Svelte SPA from $ROOM_SITE_DIR
//
// SQLite is the source of truth for threads and messages. Live deltas
// are fanned out through a tokio broadcast channel so peers never poll.
// Voice rides QUIC datagrams on the same WebTransport session and is
// never decoded server-side; the server is a dumb SFU for audio.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::Context;
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, post},
};
use clap::Parser;
use loro::LoroDoc;
use tokio::sync::{Mutex, broadcast};
use tower_http::{
    cors::{Any, CorsLayer},
    services::{ServeDir, ServeFile},
};
use wtransport::{Identity, ServerConfig};

use room_server::{
    agent,
    annotations::AnnotationMirror,
    codex_bridge,
    codex_rpc::CodexClient,
    db::Db,
    engine_claude::ClaudeEngine,
    engine_codex::CodexEngine,
    engine_handle::EngineRegistry,
    http,
    state::{AppState, WtInfo},
    wt,
};

#[derive(Parser, Debug)]
#[command(about = "Serve a multiplayer team room")]
struct Args {
    #[arg(long, env = "ROOM_HOST", default_value_t = IpAddr::V4(Ipv4Addr::UNSPECIFIED))]
    host: IpAddr,

    #[arg(long, env = "ROOM_PORT", default_value_t = 8080)]
    port: u16,

    /// UDP port the WebTransport (HTTP/3) listener binds. The advertised
    /// `https://host:wt_port` URL is what browsers and the Tauri shell
    /// pass to `new WebTransport(...)`. The Tauri client speaks only
    /// WebTransport, so the standalone server a human runs needs the
    /// listener on by default; `--no-wt` opts out for the per-run engine
    /// hosts that share a host and would collide on this fixed port.
    #[arg(long, env = "ROOM_WT_PORT", default_value_t = 4433)]
    wt_port: u16,

    /// Disable the WebTransport listener entirely. A host-placed engine
    /// host serves the HTTP `/api` surface only, so it opts out by name
    /// to avoid colliding on the fixed WebTransport UDP port across the
    /// many per-run servers that share one host.
    #[arg(long, env = "ROOM_NO_WT", default_value_t = false)]
    no_wt: bool,

    /// Hostname the WebTransport URL is published as. Defaults to
    /// `127.0.0.1` so local Chrome/WebKit clients do not resolve
    /// `localhost` to IPv6 while the default listener is IPv4.
    /// Production deployments override this so the cert SAN and the
    /// URL agree.
    #[arg(long, env = "ROOM_WT_HOST", default_value = "127.0.0.1")]
    wt_host: String,

    /// Directory containing the built Svelte assets (room-site).
    #[arg(long, env = "ROOM_SITE_DIR")]
    site_dir: Option<PathBuf>,

    /// SQLite database file. Defaults to ./room.db relative to cwd or,
    /// when set, $ROOM_STATE_DIR/room.db.
    #[arg(long, env = "ROOM_DB")]
    db: Option<PathBuf>,

    /// Directory used to derive the default db path when --db is not set.
    #[arg(long, env = "ROOM_STATE_DIR")]
    state_dir: Option<PathBuf>,

    /// Optional bearer token required for backend registration writes.
    #[arg(long, env = "ROOM_BACKEND_TOKEN")]
    backend_token: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // Site is optional now that the primary client is the Tauri
    // desktop app, which bundles the Svelte assets into its own
    // binary. When ROOM_SITE_DIR is set, we still serve / as a
    // browser-fallback for the SPA.
    let site_dir = args
        .site_dir
        .or_else(|| std::env::var_os("ROOM_SITE_DIR").map(PathBuf::from));

    let db_path = args
        .db
        .or_else(|| args.state_dir.as_ref().map(|d| d.join("room.db")))
        .unwrap_or_else(|| PathBuf::from("room.db"));

    let mut db = Db::open(&db_path)
        .with_context(|| format!("open sqlite database at {}", db_path.display()))?;
    db.migrate().context("apply sqlite migrations")?;

    // Any thread still marked 'active' or 'blocked' at boot was left
    // behind by a previous run — no codex process is working on it
    // now, so the spinner in connected clients would never clear.
    // Flip them back to 'idle' before we start serving.
    let recovered = db.reset_stuck_threads().context("reset stuck threads")?;
    if !recovered.is_empty() {
        eprintln!(
            "room-server: recovered {} stuck thread(s) on startup",
            recovered.len()
        );
    }

    // Replay the persisted Loro update log into a fresh in-memory
    // doc so the server boots already holding the room's CRDT state.
    // New WS connections get a single snapshot of this doc instead
    // of a replay loop, and a brand-new SQLite file just yields an
    // empty doc.
    let loro_doc = LoroDoc::new();
    let persisted_loro = db
        .all_loro_updates()
        .context("load persisted loro updates")?;
    let loro_update_count = persisted_loro.len();
    let mut loro_replay_skipped = 0_usize;
    for update in persisted_loro {
        if let Err(err) = loro_doc.import(&update) {
            // A poison frame from an old client version shouldn't keep
            // the server offline; log it and skip. The snapshot we
            // ship to new clients still reflects everything that did
            // import cleanly before this row.
            loro_replay_skipped += 1;
            eprintln!("room-server: skip malformed loro update during replay: {err:?}");
        }
    }
    if loro_update_count > 0 {
        eprintln!(
            "room-server: replayed {} loro update(s){}",
            loro_update_count,
            if loro_replay_skipped > 0 {
                format!(" ({loro_replay_skipped} skipped)")
            } else {
                String::new()
            }
        );
    }

    // Mirror reviewer-note annotations from the just-replayed Loro
    // doc into SQL once before serving so the read API matches the
    // doc state on cold boot (the old SQL row set might predate ops
    // that have since deleted annotations). Subsequent frames keep
    // the mirror in sync from inside the WS handler.
    let mut annotation_mirror = AnnotationMirror::new();
    if let Err(err) = annotation_mirror.sync(&loro_doc, &mut db) {
        eprintln!("room-server: failed to seed annotation mirror at boot: {err:#}");
    }

    let (tx, _) = broadcast::channel(1024);
    let (loro_tx, _) = broadcast::channel(1024);
    // Audio fan-out runs hot under continuous voice, so size the
    // channel to absorb a few seconds of 20 ms frames per peer in a
    // small room without lagging slow subscribers off the bus.
    let (audio_tx, _) = broadcast::channel::<bytes::Bytes>(4096);
    let db = Arc::new(Mutex::new(db));
    let loro_doc = Arc::new(Mutex::new(loro_doc));
    let annotation_mirror = Arc::new(Mutex::new(annotation_mirror));

    // Spin up codex app-server. If it fails (no `codex` on PATH,
    // auth missing, etc.) keep going so read-only viewer paths still
    // work. /api/chat will 503 until codex is wired up
    // correctly.
    let codex = match CodexClient::spawn().await {
        Ok(client) => {
            codex_bridge::start(
                client.clone(),
                db.clone(),
                tx.clone(),
                loro_doc.clone(),
                loro_tx.clone(),
            );
            eprintln!("room-server: codex app-server attached");
            Some(client)
        }
        Err(err) => {
            eprintln!("room-server: codex unavailable, /api/chat will 503: {err:#}");
            None
        }
    };

    // Engine-agnostic registry behind the `Engine` trait. The codex
    // adapter wraps the same live client the legacy `/api/codex/*`
    // routes use, so both paths drive one subprocess. The claude
    // adapter runs cold per-turn subprocesses, so it registers
    // unconditionally; a missing binary or `ANTHROPIC_API_KEY` surfaces
    // when a turn starts rather than at boot.
    let mut engines = EngineRegistry::new().with_claude(ClaudeEngine::new());
    if let Some(client) = codex.clone() {
        engines = engines.with_codex(CodexEngine::new(client));
    }

    // WebTransport runs unless --no-wt is set. Host-placed engine hosts
    // opt out: they serve the HTTP /api surface only, so they skip both
    // the per-boot self-signed cert and the UDP listener that otherwise
    // collided on the fixed wt_port across every per-run host server.
    let wt = if args.no_wt {
        None
    } else {
        // Self-signed cert regenerated at every boot. The cert hash
        // is pinned by every connecting client through
        // `serverCertificateHashes`, so we never need a CA in dev. The
        // WebTransport spec caps hash-pinned certs at 14 days validity,
        // which `Identity::self_signed` honors by default.
        let wt_identity = Identity::self_signed([args.wt_host.clone()])
            .context("generate self-signed wtransport cert")?;
        let cert_hash_hex = {
            let chain = wt_identity.certificate_chain();
            let cert = chain
                .as_slice()
                .first()
                .context("self-signed cert chain unexpectedly empty")?;
            hex::encode(cert.hash().as_ref())
        };

        let info = WtInfo {
            wt_url: format!("https://{}:{}", args.wt_host, args.wt_port),
            cert_hash_hex,
        };
        let config = ServerConfig::builder()
            .with_bind_address(SocketAddr::new(args.host, args.wt_port))
            .with_identity(wt_identity)
            .build();
        Some((info, config))
    };

    let wt_info = wt.as_ref().map(|(info, _)| info.clone());

    let state = AppState {
        db: db.clone(),
        backend_token: args.backend_token.filter(|s| !s.trim().is_empty()),
        broadcast: tx,
        loro_broadcast: loro_tx,
        loro_doc,
        annotation_mirror,
        codex,
        engines,
        audio_broadcast: audio_tx,
        wt_info: wt_info.clone(),
    };

    // /api/chat carries optional image attachments as base64 data
    // URLs, so a multi-image upload can comfortably exceed axum's
    // default 2 MiB JSON body limit. http::chat enforces a stricter
    // per-image cap; this just stops the extractor from rejecting
    // the request before chat() ever runs.
    const CHAT_BODY_LIMIT: usize = 256 * 1024 * 1024;

    let mut app = Router::new()
        .route("/api/wt/info", get(http::wt_info))
        .route(
            "/api/chat",
            post(http::chat).layer(DefaultBodyLimit::max(CHAT_BODY_LIMIT)),
        )
        .route(
            "/api/workflow/turns",
            post(http::workflow_turn).layer(DefaultBodyLimit::max(CHAT_BODY_LIMIT)),
        )
        // Engine-agnostic turn submission. Dispatches on the request's
        // `engine` field through the `Engine` trait; never names codex
        // or claude. The legacy `/api/codex/*` routes stay alongside.
        .route(
            "/api/agent/turns",
            post(agent::agent_turn).layer(DefaultBodyLimit::max(CHAT_BODY_LIMIT)),
        )
        .route("/api/threads", get(http::list_threads))
        .route("/api/threads/{id}", get(http::get_thread))
        .route("/api/threads/{id}/archive", post(http::archive_thread))
        .route("/api/threads/{id}/interrupt", post(http::interrupt_thread))
        .route("/api/threads/{id}/workspace", get(http::thread_workspace))
        .route(
            "/api/threads/{id}/changed-files",
            get(http::thread_changed_files),
        )
        .route("/api/threads/{id}/diff", get(http::thread_diff))
        .route("/api/threads/{id}/files", get(http::thread_files))
        .route("/api/threads/{id}/file", get(http::thread_file))
        .route(
            "/api/threads/{id}/goal",
            post(http::set_goal).delete(http::clear_goal),
        )
        .route("/api/threads/{id}/messages", get(http::list_messages))
        .route("/api/annotations", get(http::list_annotations))
        .route(
            "/api/backends",
            get(http::list_backends).post(http::upsert_backend),
        )
        .route(
            "/api/backends/{id}",
            axum::routing::delete(http::delete_backend),
        )
        .route(
            "/api/backends/{id}/proxy/api/threads",
            get(http::proxy_list_threads),
        )
        .route(
            "/api/backends/{id}/proxy/api/threads/{thread_id}",
            get(http::proxy_get_thread),
        )
        .route(
            "/api/backends/{id}/proxy/api/threads/{thread_id}/messages",
            get(http::proxy_list_messages),
        )
        .route(
            "/api/backends/{id}/proxy/api/threads/{thread_id}/archive",
            post(http::proxy_archive_thread),
        )
        .route("/api/codex/models", get(http::codex_models))
        .route(
            "/api/codex/permission-profiles",
            get(http::codex_permission_profiles),
        )
        .route("/api/codex/config", get(http::codex_config))
        .route("/api/codex/skills", get(http::codex_skills))
        .route("/api/codex/file-search", post(http::codex_file_search))
        .route(
            "/api/codex/requests/{id}/respond",
            post(http::respond_codex_request),
        )
        .route("/api/codex/requests", get(http::pending_codex_requests))
        .route("/api/health", get(http::health))
        .route("/api/loro/state", get(http::loro_state))
        .route("/api/loro/updates", get(http::loro_updates))
        .route("/api/loro/snapshot", get(http::loro_snapshot));

    if let Some(dir) = site_dir.as_ref() {
        let index = dir.join("index.html");
        app = app.fallback_service(ServeDir::new(dir).not_found_service(ServeFile::new(index)));
    }

    // The desktop client at tauri://localhost and the Vite dev server
    // at http://localhost:1420 both speak to this server cross-origin
    // once the user points the Settings → Server URL at a non-proxy
    // address. Without an explicit Access-Control-Allow-Origin the
    // browser rejects every /api response after the preflight, which
    // surfaces in the webview as "Origin … is not allowed by
    // Access-Control-Allow-Origin" and leaves the UI staring at an
    // empty thread. A wildcard origin is appropriate here: the API is
    // unauthenticated apart from the optional event-token (which uses
    // a bearer header that browsers refuse to send with `Any` origins
    // anyway), and the production deploy is gated by tailnet
    // reachability rather than browser CORS.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = app.layer(cors).with_state(state.clone());

    let addr = SocketAddr::new(args.host, args.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;

    eprintln!(
        "room-server: listening on {addr} (db={}, wt={})",
        db_path.display(),
        wt_info
            .as_ref()
            .map_or("disabled", |info| info.wt_url.as_str()),
    );

    let wt_shutdown = wt.map(|(_, wt_config)| {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let wt_state = state.clone();
        let handle = tokio::spawn(async move {
            let shutdown = async {
                let _ = rx.await;
            };
            if let Err(err) = wt::serve(wt_config, wt_state, shutdown).await {
                eprintln!("room-server: wtransport listener failed: {err:#}");
            }
        });
        (tx, handle)
    });

    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("room-server failed");

    if let Some((wt_shutdown_tx, wt_handle)) = wt_shutdown {
        let _ = wt_shutdown_tx.send(());
        let _ = wt_handle.await;
    }

    serve_result
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    // Give the broadcast channel a moment to drain to subscribers
    // before the server tears down.
    tokio::time::sleep(Duration::from_millis(50)).await;
}
