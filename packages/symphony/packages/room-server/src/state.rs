// Shared server state.
//
// We hand the `AppState` to every axum handler via `with_state`. The
// SQLite connection is wrapped in a tokio `Mutex` because `rusqlite`
// is not Sync, and the broadcast channels fan deltas out to every
// connected websocket.

use std::sync::Arc;

use bytes::Bytes;
use loro::LoroDoc;
use tokio::sync::{Mutex, broadcast};

use crate::{
    annotations::AnnotationMirror, codex_bridge::Delta, codex_rpc::CodexClient, db::Db,
    engine_handle::EngineRegistry,
};

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Db>>,
    pub backend_token: Option<String>,
    pub broadcast: broadcast::Sender<Delta>,
    /// Loro CRDT update frames, relayed between every connected
    /// websocket. Carried out-of-band from the JSON delta channel so
    /// the wire stays binary; persisted to `loro_updates` by the WS
    /// handler so the room's CRDT state survives restarts.
    pub loro_broadcast: broadcast::Sender<Vec<u8>>,
    /// In-memory authoritative copy of the room's Loro doc. Every
    /// accepted binary frame is imported here before persistence so
    /// new connections can be handed a single snapshot instead of
    /// replaying the whole log over the socket. Use `parking_lot`'s
    /// Mutex would be lighter, but `tokio::sync::Mutex` keeps the
    /// hold-across-await story simple if we ever need it.
    pub loro_doc: Arc<Mutex<LoroDoc>>,
    /// SQL mirror of reviewer-note annotations stored in the Loro
    /// doc. The Loro doc is canonical; this is the queryable side-
    /// table that lets retroactive AGENTS.md mining `SELECT … JOIN
    /// messages` without rehydrating the CRDT. Reconciled by
    /// `annotations::AnnotationMirror::sync` after every accepted
    /// Loro frame and once at boot.
    pub annotation_mirror: Arc<Mutex<AnnotationMirror>>,
    /// Live JSON-RPC handle to a `codex app-server` subprocess. None
    /// means the server started without a working codex binary, which
    /// is allowed for read-only deploys but causes chat endpoints to 503.
    ///
    /// This is the legacy concrete-engine handle the `/api/codex/*`
    /// routes and the codex bridge still use. New work goes through
    /// `engines` below; the cutover that removes this field is a later
    /// integration workstream.
    pub codex: Option<Arc<CodexClient>>,
    /// Engine-agnostic registry behind the `Engine` trait. The new
    /// `/api/agent/*` path looks an engine up here by the `engine` field
    /// on a `TurnRequest` and never names a concrete adapter.
    pub engines: EngineRegistry,
    /// Per-room voice fan-out. Each datagram carries an 8-byte
    /// big-endian sender peer id followed by an opaque Opus packet.
    /// The server never inspects the audio bytes; it just routes a
    /// datagram to every other peer in the room. Receivers compare
    /// the sender id against their own and drop self-echoes.
    pub audio_broadcast: broadcast::Sender<Bytes>,
    /// WebTransport connection coordinates exposed to clients via
    /// `/api/wt/info`. The cert hash is the sha-256 of the DER cert
    /// the WebTransport listener is using, formatted as lowercase
    /// hex; clients feed it into the
    /// `serverCertificateHashes` constructor option so a self-signed
    /// dev cert is accepted without a CA. `wt_url` is the
    /// `https://host:port` endpoint browsers should pass to
    /// `new WebTransport(...)`. `None` when the server runs as a
    /// host-placed engine host that serves the HTTP `/api` surface only
    /// and binds no WebTransport listener.
    pub wt_info: Option<WtInfo>,
}

#[derive(Clone, Debug)]
pub struct WtInfo {
    pub wt_url: String,
    pub cert_hash_hex: String,
}
