//! The Loro sync protocol over a websocket, so loro-dev's own browser client
//! can join the dashboard document.
//!
//! `/events` plus `POST /apply` already sync the same [`Hub`], but they are
//! this repo's own shapes and nothing off the shelf speaks them. This endpoint
//! speaks the published wire format instead, so a page that does
//! `client.join(doc, "dashboard")` with the `loro-websocket` package syncs
//! against the hub with no adapter in between. Both transports read the one
//! broadcast, so a browser on SSE and a browser on the websocket converge on
//! identical bytes.
//!
//! One frame per binary websocket message, framed
//! `[4] magic | [varUint] roomIdLen | roomId | [1] type | payload`;
//! [`loro_protocol`] owns that encoding (see the note on the workspace
//! dependency for why it is a dependency rather than varints written out here).
//! Keepalive is deliberately outside it: the client sends a websocket **text**
//! frame `"ping"` and expects text `"pong"`, and a server that answers in
//! binary -- or not at all -- gets a full client reconnect every 40 seconds.
//!
//! The hub is one document, so this serves exactly one room ([`ROOM_ID`]) under
//! the `%LOR` magic. A join naming anything else is refused rather than aliased
//! onto the same document: rooms key on (magic, room id) by the spec, and two
//! clients that believe they are in different rooms while sharing one document
//! is the failure that reads as data corruption from the outside.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket};
use loro_protocol::{
    BatchId, CrdtType, JoinErrorCode, MAX_MESSAGE_SIZE, Permission, ProtocolMessage, RoomErrorCode,
    UpdateStatusCode, encode, try_decode,
};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::hub::{Hub, Merge};

/// The one room this server serves. The hub owns a single document, so a room
/// id names that document rather than selecting among several.
pub const ROOM_ID: &str = "dashboard";

/// Largest CRDT update carried in one `DocUpdate`. The whole frame has to fit
/// `MAX_MESSAGE_SIZE`, and the magic, room id, type tag and 8-byte batch id
/// ride along with it, so this leaves the same headroom the official client
/// leaves when it splits its own sends.
const FRAGMENT_LIMIT: usize = 240 * 1024;

/// Ceiling on a client's declared `fragment_count`. The count sizes a slot
/// vector before a single fragment has arrived, so it is a number the client
/// picks and this process pays for.
const MAX_FRAGMENTS: u64 = 4096;

/// Ceiling on a client's declared `total_size_bytes`, for the same reason.
const MAX_BATCH_BYTES: u64 = 64 * 1024 * 1024;

/// How long a half-delivered fragment batch is held before it is dropped and
/// acked `fragment_timeout`. Without it, a client that opens a header and then
/// stops leaves its slot vector resident for the life of the connection -- the
/// reference Rust server omits this timeout and leaks in exactly that way.
const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(10);

/// Depth of the relay's fan-out ring, in hub deltas. Matched to the hub's own
/// broadcast depth: a connection that outruns this ring has outrun the hub's
/// too, and the answer is a fresh snapshot either way.
const RELAY_CAPACITY: usize = 256;

/// The wire frames one CRDT update turns into: a lone `DocUpdate`, or a
/// fragment header followed by its fragments.
///
/// Shared by reference across every joined connection. Re-encoding per
/// recipient would be N times the work for byte-identical output, and two
/// encodes of one update is two chances to disagree.
pub type Frames = Arc<Vec<Bytes>>;

/// Batch ids for server-originated updates.
///
/// The client keys fragment reassembly on (magic, room id, batch id), so two
/// batches in flight to one client must not collide. A process-wide counter is
/// enough: ids are never compared across connections, and this one is monotone.
static NEXT_BATCH: AtomicU64 = AtomicU64::new(1);

fn next_batch() -> BatchId {
    BatchId(NEXT_BATCH.fetch_add(1, Ordering::Relaxed).to_be_bytes())
}

/// Start the fan-out task and hand back the channel joined connections read.
///
/// One task encodes each hub delta once for every websocket client, which is
/// what keeps the relay verbatim: each recipient gets the same `Bytes`, not its
/// own re-encoding. The caller attaches the returned handle to the dashboard so
/// shutdown winds it down with the server.
/// The running relay: the channel new sockets subscribe to, and the task that
/// feeds it. The caller has to keep both -- dropping the handle without winding
/// the task down leaks it past shutdown.
pub struct Relay {
    pub frames: broadcast::Sender<Frames>,
    pub task: JoinHandle<()>,
}

pub fn start_relay(hub: Arc<Hub>, runtime: &tokio::runtime::Handle) -> Relay {
    let (frames, _) = broadcast::channel(RELAY_CAPACITY);
    let task = {
        let frames = frames.clone();
        runtime.spawn(async move {
            let mut updates = hub.updates();
            loop {
                let batch = match updates.recv().await {
                    // Nobody is on the websocket. Keep draining -- a receiver
                    // that stops reading becomes the laggard -- but skip the
                    // encode: the SSE-only dashboard is the common case and
                    // should not pay for a transport it is not using.
                    Ok(_) if frames.receiver_count() == 0 => continue,
                    Ok(update) => encode_update(&update.bytes),
                    // The relay fell behind the hub, so the deltas it skipped
                    // are gone from the ring. A snapshot is the only thing that
                    // puts every joined client back on the document.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        encode_update(&hub.export_snapshot())
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if let Some(batch) = batch {
                    let _ = frames.send(batch);
                }
            }
        })
    };
    Relay { frames, task }
}

/// One connection's protocol state.
struct Session {
    /// The relay subscription, live only once the client has joined. `None`
    /// before a join is what refuses an update from a client that never
    /// introduced itself.
    joined: Option<broadcast::Receiver<Frames>>,
    /// In-flight fragment batches, keyed by the id the client chose.
    ///
    /// Per connection, which is what binds a batch to its sender for free:
    /// another connection reusing the same id gets its own entry and cannot
    /// append to, complete, or cancel this one.
    batches: HashMap<BatchId, Batch>,
}

/// A fragment batch being reassembled.
struct Batch {
    /// One slot per declared fragment, filled by index.
    chunks: Vec<Option<Vec<u8>>>,
    /// How many slots are filled, so completion is a comparison rather than a
    /// scan on every fragment.
    received: usize,
    /// The size the header declared. Fragments that overrun it are refused
    /// rather than trusted, since the header is client-supplied.
    total: usize,
    /// Bytes accepted so far, checked against `total`.
    accepted: usize,
    /// When this batch stops being worth holding.
    deadline: Instant,
}

/// Why the select loop woke.
///
/// The branch bodies cannot touch the session, because `select!` keeps the
/// other branches' futures alive across them and one of those futures holds
/// `&mut session.joined`. Waking with a value and acting after the macro
/// returns keeps every borrow inside the macro.
enum Wake {
    Incoming(Option<Result<Message, axum::Error>>),
    Relayed(Result<Frames, broadcast::error::RecvError>),
    Expired,
}

/// Speak the protocol on one connection until the client goes away.
#[allow(
    clippy::match_same_arms,
    reason = "each arm documents a distinct cause; merging them would delete the comment that explains why"
)]
pub async fn serve_socket(mut socket: WebSocket, hub: Arc<Hub>, relay: broadcast::Sender<Frames>) {
    let mut session = Session {
        joined: None,
        batches: HashMap::new(),
    };

    loop {
        // Recomputed each pass rather than cached: a connection holds a handful
        // of batches at most, and a stored deadline would need invalidating on
        // every insert, completion and expiry.
        let deadline = session.batches.values().map(|batch| batch.deadline).min();
        let wake = tokio::select! {
            incoming = socket.recv() => Wake::Incoming(incoming),
            relayed = next_relayed(session.joined.as_mut()) => Wake::Relayed(relayed),
            () = expire_at(deadline) => Wake::Expired,
        };

        let alive = match wake {
            Wake::Incoming(None | Some(Err(_))) => false,
            Wake::Incoming(Some(Ok(message))) => {
                handle(&mut socket, &mut session, &hub, &relay, message).await
            }
            Wake::Relayed(Ok(frames)) => send_frames(&mut socket, &frames).await,
            // This connection outran the relay ring, so the frames it skipped
            // are gone. Resubscribe before snapshotting, never after, for the
            // same reason a join does: a delta in between arrives twice (a Loro
            // import is idempotent) but cannot be missed.
            Wake::Relayed(Err(broadcast::error::RecvError::Lagged(_))) => {
                session.joined = Some(relay.subscribe());
                resend_snapshot(&mut socket, &hub).await
            }
            Wake::Relayed(Err(broadcast::error::RecvError::Closed)) => false,
            Wake::Expired => expire(&mut socket, &mut session).await,
        };
        if !alive {
            break;
        }
    }
}

/// Wait for the next relayed batch, or forever when this connection has not
/// joined. `select!` needs a future on every arm, and `pending` is the arm that
/// can never fire.
async fn next_relayed(
    joined: Option<&mut broadcast::Receiver<Frames>>,
) -> Result<Frames, broadcast::error::RecvError> {
    match joined {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// Sleep until the earliest reassembly deadline, or forever when no batch is in
/// flight -- which beats waking on a fixed tick to find nothing to do.
async fn expire_at(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Act on one inbound websocket message. `false` means drop the connection.
async fn handle(
    socket: &mut WebSocket,
    session: &mut Session,
    hub: &Arc<Hub>,
    relay: &broadcast::Sender<Frames>,
    message: Message,
) -> bool {
    match message {
        // Keepalive rides text frames, out of band from the binary protocol.
        // The client closes the socket when a pong does not come back, so this
        // is the difference between a stable connection and a reconnect loop.
        Message::Text(text) => {
            if text.as_str() == "ping" {
                return socket.send(Message::Text("pong".into())).await.is_ok();
            }
            true
        }
        Message::Binary(data) => protocol(socket, session, hub, relay, &data).await,
        Message::Close(_) => false,
        // A websocket Ping is answered by axum itself, and a Pong needs no
        // reply. Neither is the protocol's keepalive, which is text.
        Message::Ping(_) | Message::Pong(_) => true,
    }
}

/// Decode one binary frame and act on it.
#[allow(
    clippy::match_same_arms,
    reason = "each arm documents a distinct cause; merging them would delete the comment that explains why"
)]
async fn protocol(
    socket: &mut WebSocket,
    session: &mut Session,
    hub: &Arc<Hub>,
    relay: &broadcast::Sender<Frames>,
    data: &Bytes,
) -> bool {
    let Some(message) = try_decode(data) else {
        // Undecodable bytes carry no batch id to ack against and no room to
        // scope a RoomError to, so there is nothing to answer with. The
        // connection stays: a client that framed one message wrongly can still
        // frame the next one right.
        return true;
    };
    match message {
        ProtocolMessage::JoinRequest { crdt, room_id, .. } => {
            join(socket, session, hub, relay, crdt, room_id).await
        }
        ProtocolMessage::DocUpdate {
            crdt,
            room_id,
            updates,
            batch_id,
        } => {
            doc_update(
                socket,
                session,
                hub,
                crdt,
                &room_id,
                &updates,
                batch_id,
                data.len(),
            )
            .await
        }
        ProtocolMessage::DocUpdateFragmentHeader {
            crdt,
            room_id,
            batch_id,
            fragment_count,
            total_size_bytes,
        } => {
            fragment_header(
                socket,
                session,
                crdt,
                &room_id,
                batch_id,
                fragment_count,
                total_size_bytes,
            )
            .await
        }
        ProtocolMessage::DocUpdateFragment {
            crdt,
            room_id,
            batch_id,
            index,
            fragment,
        } => {
            take_fragment(
                socket, session, hub, crdt, &room_id, batch_id, index, fragment,
            )
            .await
        }
        ProtocolMessage::Leave { crdt, room_id } => {
            if serves(crdt, &room_id) {
                // Leaving ends the room membership, not the connection: the
                // client may rejoin on the same socket, and closing here would
                // read as a network failure and start its reconnect backoff.
                session.joined = None;
                session.batches.clear();
            }
            true
        }
        // The client acks only to report a fragment batch it gave up
        // reassembling. Nothing outbound is buffered for a resend, so the
        // recovery is a fresh snapshot; without it a client that lost one
        // snapshot stays blank until the next edit happens to arrive.
        ProtocolMessage::Ack { status, .. } => {
            if session.joined.is_some() && matches!(status, UpdateStatusCode::FragmentTimeout) {
                return resend_snapshot(socket, hub).await;
            }
            true
        }
        // A client sends JoinError to report that it could not decode something
        // this server said -- "Invalid version format received" is the one seen
        // in practice. Undocumented but real, so it is tolerated rather than
        // treated as a protocol violation. It is not logged because this crate
        // links into the TUI process, where stderr is the terminal the dashboard
        // is drawn on.
        ProtocolMessage::JoinError { .. } => true,
        // Server-to-client messages; a client that sends one is confused, but
        // not in a way that costs this side anything.
        ProtocolMessage::JoinResponseOk { .. } | ProtocolMessage::RoomError { .. } => true,
    }
}

/// Whether a frame addresses the room this server serves.
///
/// `%EPH` / `%EPS` presence rooms and any other room id are distinct rooms by
/// the spec's own keying, and the hub has nothing behind them. Answering as if
/// it did would hand a client an empty document that silently never syncs.
fn serves(crdt: CrdtType, room_id: &str) -> bool {
    matches!(crdt, CrdtType::Loro) && room_id == ROOM_ID
}

/// Refuse a batch-carrying frame that arrived before a successful join, or for
/// a room this server does not serve.
///
/// Every such frame is refused identically, so the rule lives here once rather
/// than at each handler's head. `Some` is the handler's return value (whether
/// the Ack reached the socket); `None` admits the frame.
async fn refuse_unjoined(
    socket: &mut WebSocket,
    session: &Session,
    crdt: CrdtType,
    room_id: &str,
    batch_id: BatchId,
) -> Option<bool> {
    if session.joined.is_some() && serves(crdt, room_id) {
        return None;
    }
    Some(
        ack(
            socket,
            room_id,
            batch_id,
            UpdateStatusCode::PermissionDenied,
        )
        .await,
    )
}

/// Admit a client to the room and seed it with the current document.
async fn join(
    socket: &mut WebSocket,
    session: &mut Session,
    hub: &Arc<Hub>,
    relay: &broadcast::Sender<Frames>,
    crdt: CrdtType,
    room_id: String,
) -> bool {
    if !serves(crdt, &room_id) {
        return send(
            socket,
            &ProtocolMessage::JoinError {
                crdt,
                room_id,
                code: JoinErrorCode::AppError,
                message: format!("this server serves only the `{ROOM_ID}` Loro document room"),
                receiver_version: None,
                app_code: Some("unknown_room".to_owned()),
            },
        )
        .await;
    }

    // Subscribe before snapshotting, never after: a delta landing between the
    // two then arrives twice, which a Loro import absorbs, instead of falling
    // in a gap, which would leave the client one edit behind forever.
    session.joined = Some(relay.subscribe());
    let snapshot = hub.export_snapshot();

    // Three details the client is unforgiving about.
    //
    // `permission` must decode as exactly "read" or "write"; anything else is a
    // fatal decode error whose join promise neither resolves nor rejects. Write
    // for everyone, matching `/apply`, which is equally unauthenticated on this
    // same origin.
    //
    // `extra` is empty here but mandatory on the wire -- an absent one fails the
    // client's decoder the same fatal way.
    //
    // The zero-length version is the protocol's lazy path: the client reads it
    // as "the server knows nothing" and pushes its whole document, so no version
    // vector is decoded on this side at all. Answering `version_unknown`
    // instead would be wrong even for an empty version, because the client's
    // recovery for that code is to retry with the empty version -- an unbroken
    // join loop.
    if !send(
        socket,
        &ProtocolMessage::JoinResponseOk {
            crdt,
            room_id,
            permission: Permission::Write,
            version: Vec::new(),
            extra: Some(Vec::new()),
        },
    )
    .await
    {
        return false;
    }
    send_update(socket, &snapshot).await
}

/// Merge a client's `DocUpdate` and answer it.
#[allow(
    clippy::too_many_arguments,
    reason = "one protocol frame, one argument each"
)]
async fn doc_update(
    socket: &mut WebSocket,
    session: &Session,
    hub: &Arc<Hub>,
    crdt: CrdtType,
    room_id: &str,
    updates: &[Vec<u8>],
    batch_id: BatchId,
    frame_len: usize,
) -> bool {
    if let Some(refused) = refuse_unjoined(socket, session, crdt, room_id, batch_id).await {
        return refused;
    }
    // The limit binds the whole frame, and a client that overshoots it has a
    // fragmenting bug rather than a bad update, so it gets its own code.
    if frame_len > MAX_MESSAGE_SIZE {
        return ack(socket, room_id, batch_id, UpdateStatusCode::PayloadTooLarge).await;
    }
    let outcome = merge(hub, updates.iter().map(Vec::as_slice));
    answer(socket, room_id, batch_id, outcome).await
}

/// Open a fragment batch.
#[allow(
    clippy::too_many_arguments,
    reason = "one protocol frame, one argument each"
)]
async fn fragment_header(
    socket: &mut WebSocket,
    session: &mut Session,
    crdt: CrdtType,
    room_id: &str,
    batch_id: BatchId,
    fragment_count: u64,
    total_size_bytes: u64,
) -> bool {
    if let Some(refused) = refuse_unjoined(socket, session, crdt, room_id, batch_id).await {
        return refused;
    }
    // Both numbers are the client's to claim and this process's to allocate
    // against, so they are bounded before either is believed.
    if fragment_count == 0 || fragment_count > MAX_FRAGMENTS || total_size_bytes > MAX_BATCH_BYTES {
        return ack(socket, room_id, batch_id, UpdateStatusCode::InvalidUpdate).await;
    }
    let (Ok(count), Ok(total)) = (
        usize::try_from(fragment_count),
        usize::try_from(total_size_bytes),
    ) else {
        return ack(socket, room_id, batch_id, UpdateStatusCode::InvalidUpdate).await;
    };
    // A repeated header restarts the batch rather than adding to it: a client
    // only re-sends a header after giving up on its first attempt.
    session.batches.insert(
        batch_id,
        Batch {
            chunks: vec![None; count],
            received: 0,
            total,
            accepted: 0,
            deadline: Instant::now() + REASSEMBLY_TIMEOUT,
        },
    );
    true
}

/// Take one fragment, and merge the batch once it completes.
#[allow(
    clippy::too_many_arguments,
    reason = "one protocol frame, one argument each"
)]
async fn take_fragment(
    socket: &mut WebSocket,
    session: &mut Session,
    hub: &Arc<Hub>,
    crdt: CrdtType,
    room_id: &str,
    batch_id: BatchId,
    index: u64,
    fragment: Vec<u8>,
) -> bool {
    if let Some(refused) = refuse_unjoined(socket, session, crdt, room_id, batch_id).await {
        return refused;
    }
    let Some(batch) = session.batches.get_mut(&batch_id) else {
        // No header, or a batch this connection already expired. `invalid_update`
        // rather than `fragment_timeout`, because the expiry sweep has already
        // sent the timeout for that id and a second one would ask the client to
        // resend a batch it is in the middle of resending.
        return ack(socket, room_id, batch_id, UpdateStatusCode::InvalidUpdate).await;
    };
    let slot = usize::try_from(index)
        .ok()
        .filter(|at| *at < batch.chunks.len());
    let Some(slot) = slot else {
        session.batches.remove(&batch_id);
        return ack(socket, room_id, batch_id, UpdateStatusCode::InvalidUpdate).await;
    };
    if batch.chunks[slot].is_none() {
        batch.accepted += fragment.len();
        // The header's total is what the reassembly buffer is sized from, so
        // fragments that outrun it are a lie about how much memory this batch
        // costs, not a recoverable off-by-one.
        if batch.accepted > batch.total {
            session.batches.remove(&batch_id);
            return ack(socket, room_id, batch_id, UpdateStatusCode::InvalidUpdate).await;
        }
        batch.received += 1;
        batch.chunks[slot] = Some(fragment);
    }
    if batch.received < batch.chunks.len() {
        // Nothing is acked mid-batch: one Ack per batch id is what the client
        // matches against the copy it is holding.
        return true;
    }

    let Some(batch) = session.batches.remove(&batch_id) else {
        return true;
    };
    let mut update = Vec::with_capacity(batch.accepted);
    for chunk in batch.chunks.into_iter().flatten() {
        update.extend_from_slice(&chunk);
    }
    let outcome = merge(hub, std::iter::once(update.as_slice()));
    answer(socket, room_id, batch_id, outcome).await
}

/// Drop every batch whose reassembly window has closed, acking each so the
/// client stops holding the copy it is waiting to hear about.
async fn expire(socket: &mut WebSocket, session: &mut Session) -> bool {
    let now = Instant::now();
    let stale: Vec<BatchId> = session
        .batches
        .iter()
        .filter(|(_, batch)| batch.deadline <= now)
        .map(|(id, _)| *id)
        .collect();
    for id in stale {
        session.batches.remove(&id);
        if !ack(socket, ROOM_ID, id, UpdateStatusCode::FragmentTimeout).await {
            return false;
        }
    }
    true
}

/// What a merged batch means for the client.
enum Outcome {
    /// Every op applied and is on its way back out through the relay.
    Applied,
    /// Loro recorded the ops but could not apply them: they depend on history
    /// this document has never seen. Not the client's mistake, and the protocol
    /// has no status for it, so it is acked ok -- the bytes *were* taken -- and
    /// followed by `rejoin_suggested`, which is how the protocol asks a peer for
    /// its document again.
    Pending,
    /// The bytes are not a Loro update.
    Invalid,
}

/// Merge a client's updates into the hub. Each is imported on its own so one
/// bad update in a batch is not blamed on the ones before it, which already
/// landed and were fanned out.
fn merge<'a>(hub: &Hub, updates: impl Iterator<Item = &'a [u8]>) -> Outcome {
    let mut outcome = Outcome::Applied;
    for update in updates {
        match hub.import(update) {
            Ok(Merge::Applied) => {}
            Ok(Merge::Pending) => outcome = Outcome::Pending,
            Err(_) => return Outcome::Invalid,
        }
    }
    outcome
}

/// Answer a merged batch: the Ack it always gets, plus a rejoin nudge when the
/// document could not absorb it.
async fn answer(
    socket: &mut WebSocket,
    room_id: &str,
    batch_id: BatchId,
    outcome: Outcome,
) -> bool {
    let status = match outcome {
        Outcome::Applied | Outcome::Pending => UpdateStatusCode::Ok,
        Outcome::Invalid => UpdateStatusCode::InvalidUpdate,
    };
    if !ack(socket, room_id, batch_id, status).await {
        return false;
    }
    if matches!(outcome, Outcome::Pending) {
        return send(
            socket,
            &ProtocolMessage::RoomError {
                crdt: CrdtType::Loro,
                room_id: room_id.to_owned(),
                code: RoomErrorCode::RejoinSuggested,
                message: "update depends on ops this document has never seen".to_owned(),
            },
        )
        .await;
    }
    true
}

/// Acknowledge one inbound batch.
///
/// Every batch is acked, successes included. The client keeps a copy of each
/// batch it sent until an Ack for that id releases it, so a server that acks
/// only failures grows the browser's memory for as long as the session lasts.
async fn ack(
    socket: &mut WebSocket,
    room_id: &str,
    batch_id: BatchId,
    status: UpdateStatusCode,
) -> bool {
    send(
        socket,
        &ProtocolMessage::Ack {
            crdt: CrdtType::Loro,
            room_id: room_id.to_owned(),
            ref_id: batch_id,
            status,
        },
    )
    .await
}

/// Export the document afresh and push it. The recovery for a client whose view
/// this side can no longer reconstruct incrementally.
async fn resend_snapshot(socket: &mut WebSocket, hub: &Arc<Hub>) -> bool {
    let snapshot = hub.export_snapshot();
    send_update(socket, &snapshot).await
}

/// Frame one CRDT update and send it.
async fn send_update(socket: &mut WebSocket, update: &[u8]) -> bool {
    let Some(frames) = encode_update(update) else {
        return true;
    };
    send_frames(socket, &frames).await
}

async fn send_frames(socket: &mut WebSocket, frames: &[Bytes]) -> bool {
    for frame in frames {
        if socket.send(Message::Binary(frame.clone())).await.is_err() {
            return false;
        }
    }
    true
}

/// Encode one message, or `None` when it does not fit the wire.
///
/// The only way `encode` refuses a message is on size, and every caller here
/// either fits by construction (an Ack, a join answer) or was fragmented to fit
/// by [`encode_update`], so this is a total function in practice. It stays
/// fallible rather than panicking because the alternative to dropping one frame
/// is killing the process that draws the dashboard.
fn frame(message: &ProtocolMessage) -> Option<Bytes> {
    encode(message).ok().map(Bytes::from)
}

/// Encode and send one message. `false` means the socket is gone.
async fn send(socket: &mut WebSocket, message: &ProtocolMessage) -> bool {
    let Some(bytes) = frame(message) else {
        return true;
    };
    socket.send(Message::Binary(bytes)).await.is_ok()
}

/// Encode one CRDT update as the frames that carry it: a single `DocUpdate`
/// when it fits, otherwise a fragment header followed by its fragments.
///
/// Fragmenting outbound is not optional once a document grows: `encode` refuses
/// any frame over `MAX_MESSAGE_SIZE`, so an unfragmented snapshot past the limit
/// would be dropped here and the client would sit on an empty document with
/// nothing to retry and no error to show.
fn encode_update(update: &[u8]) -> Option<Frames> {
    if update.is_empty() {
        return None;
    }
    let batch_id = next_batch();
    if update.len() <= FRAGMENT_LIMIT {
        return frame(&ProtocolMessage::DocUpdate {
            crdt: CrdtType::Loro,
            room_id: ROOM_ID.to_owned(),
            updates: vec![update.to_vec()],
            batch_id,
        })
        .map(|frame| Arc::new(vec![frame]));
    }

    let chunks: Vec<&[u8]> = update.chunks(FRAGMENT_LIMIT).collect();
    let mut frames = Vec::with_capacity(chunks.len() + 1);
    frames.push(frame(&ProtocolMessage::DocUpdateFragmentHeader {
        crdt: CrdtType::Loro,
        room_id: ROOM_ID.to_owned(),
        batch_id,
        fragment_count: u64::try_from(chunks.len()).ok()?,
        total_size_bytes: u64::try_from(update.len()).ok()?,
    })?);
    for (index, chunk) in chunks.into_iter().enumerate() {
        frames.push(frame(&ProtocolMessage::DocUpdateFragment {
            crdt: CrdtType::Loro,
            room_id: ROOM_ID.to_owned(),
            batch_id,
            index: u64::try_from(index).ok()?,
            fragment: chunk.to_vec(),
        })?);
    }
    Some(Arc::new(frames))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use futures::{SinkExt as _, StreamExt as _};
    use loro::{ExportMode, LoroDoc, LoroValue};
    use loro_protocol::decode;
    use tokio::net::TcpStream;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

    use super::super::server::{ServedDashboard, serve_hub};
    use super::*;

    /// Offset of the type byte in every frame: 4 magic bytes, the one-byte
    /// length prefix of a 9-byte room id, then the id itself.
    const TYPE_AT: usize = 4 + 1 + ROOM_ID.len();

    type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

    /// A dashboard on a real ephemeral port. The handle is returned because
    /// dropping it aborts the server.
    async fn dashboard() -> ServedDashboard {
        serve_hub(
            Hub::new(),
            SocketAddr::from(([127, 0, 0, 1], 0)),
            None,
            &tokio::runtime::Handle::current(),
        )
        .await
        .expect("serve the dashboard")
    }

    async fn connect(addr: SocketAddr) -> Client {
        let (client, _) = connect_async(format!("ws://{addr}/ws"))
            .await
            .expect("upgrade to the sync protocol");
        client
    }

    async fn send(client: &mut Client, message: &ProtocolMessage) {
        let bytes = encode(message).expect("encode the frame");
        client
            .send(WsMessage::Binary(bytes.into()))
            .await
            .expect("send the frame");
    }

    /// The next frame, with a deadline so a server that answers nothing fails
    /// the test instead of hanging it.
    async fn next(client: &mut Client) -> WsMessage {
        tokio::time::timeout(Duration::from_secs(5), client.next())
            .await
            .expect("a frame within 5s")
            .expect("the stream to still be open")
            .expect("a readable frame")
    }

    async fn next_binary(client: &mut Client) -> Vec<u8> {
        match next(client).await {
            WsMessage::Binary(bytes) => bytes.to_vec(),
            other => panic!("expected a binary frame, got {other:?}"),
        }
    }

    async fn next_message(client: &mut Client) -> ProtocolMessage {
        let bytes = next_binary(client).await;
        decode(&bytes).expect("a decodable protocol frame")
    }

    fn join_request(room_id: &str) -> ProtocolMessage {
        ProtocolMessage::JoinRequest {
            crdt: CrdtType::Loro,
            room_id: room_id.to_owned(),
            auth: Vec::new(),
            version: Vec::new(),
        }
    }

    /// Join the document room and return the seed snapshot the server pushes
    /// straight after the response.
    async fn join(client: &mut Client) -> Vec<u8> {
        send(client, &join_request(ROOM_ID)).await;
        let response = next_binary(client).await;
        assert_eq!(response[TYPE_AT], 0x01, "the join was refused");
        match next_message(client).await {
            ProtocolMessage::DocUpdate { updates, .. } => {
                updates.into_iter().next().expect("one seeded update")
            }
            other => panic!("expected the seed snapshot, got {other:?}"),
        }
    }

    /// An update from a document this hub has never seen, so it merges cleanly.
    fn independent_update(title: &str) -> Vec<u8> {
        let doc = LoroDoc::new();
        doc.get_map("panel").insert("title", title).expect("write");
        doc.commit();
        doc.export(ExportMode::all_updates())
            .expect("export the update")
    }

    fn title_of(doc: &LoroDoc) -> Option<String> {
        match doc.get_map("panel").get("title")?.get_deep_value() {
            LoroValue::String(text) => Some(text.to_string()),
            _ => None,
        }
    }

    /// The three fields the official client is fatal about, pinned as bytes.
    ///
    /// `permission` decodes as exactly "read" or "write" or the client throws
    /// inside its decoder, leaving the join promise neither resolved nor
    /// rejected; `extra` is mandatory even when empty; and the zero-length
    /// version is the lazy path that makes the client push its whole document.
    /// Bytes rather than a decode-and-compare because these are the exact
    /// positions a client's decoder walks.
    #[tokio::test]
    async fn a_join_response_pins_write_permission_an_empty_version_and_a_present_extra() {
        let served = dashboard().await;
        let mut client = connect(served.dashboard.addr()).await;

        send(&mut client, &join_request(ROOM_ID)).await;
        let frame = next_binary(&mut client).await;

        assert_eq!(&frame[..4], b"%LOR", "document magic");
        assert_eq!(
            usize::from(frame[4]),
            ROOM_ID.len(),
            "room id length prefix"
        );
        assert_eq!(&frame[5..TYPE_AT], ROOM_ID.as_bytes());
        assert_eq!(frame[TYPE_AT], 0x01, "JoinResponseOk");
        assert_eq!(
            &frame[TYPE_AT + 1..],
            &[0x05, b'w', b'r', b'i', b't', b'e', 0x00, 0x00],
            "permission `write`, then an empty version, then an empty but present extra"
        );
    }

    /// The round trip the whole endpoint exists for: one client's update is
    /// acked and shows up in another client's document.
    #[tokio::test]
    async fn an_update_is_acked_ok_and_relayed_to_the_other_client() {
        let served = dashboard().await;
        let addr = served.dashboard.addr();
        let mut alice = connect(addr).await;
        let mut bob = connect(addr).await;
        join(&mut alice).await;
        let seed = join(&mut bob).await;

        let batch_id = BatchId([9, 8, 7, 6, 5, 4, 3, 2]);
        send(
            &mut alice,
            &ProtocolMessage::DocUpdate {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                updates: vec![independent_update("relayed")],
                batch_id,
            },
        )
        .await;

        // The Ack lands before the echo of the relay, because the connection
        // task is inside the inbound branch until the Ack is written.
        assert_eq!(
            next_message(&mut alice).await,
            ProtocolMessage::Ack {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                ref_id: batch_id,
                status: UpdateStatusCode::Ok,
            },
            "every batch is acked, successes included"
        );

        let ProtocolMessage::DocUpdate { updates, .. } = next_message(&mut bob).await else {
            panic!("expected the relayed update on the second client");
        };
        // Bob's document is the join seed plus the relayed delta. The title only
        // appears if both arrived and merged, which is the real assertion: a
        // frame that decoded but carried the wrong bytes would fail here.
        let mirror = LoroDoc::new();
        mirror.import(&seed).expect("import the seed");
        for update in &updates {
            mirror.import(update).expect("import the relayed delta");
        }
        assert_eq!(title_of(&mirror).as_deref(), Some("relayed"));
    }

    /// An update from a client that never joined is refused, and refused with
    /// the code the protocol reserves for it rather than by silently ignoring
    /// the frame -- an unacked batch is one the client holds forever.
    #[tokio::test]
    async fn an_update_before_joining_is_refused_with_permission_denied() {
        let served = dashboard().await;
        let mut client = connect(served.dashboard.addr()).await;

        let batch_id = BatchId([1; 8]);
        send(
            &mut client,
            &ProtocolMessage::DocUpdate {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                updates: vec![independent_update("unjoined")],
                batch_id,
            },
        )
        .await;

        assert_eq!(
            next_message(&mut client).await,
            ProtocolMessage::Ack {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                ref_id: batch_id,
                status: UpdateStatusCode::PermissionDenied,
            }
        );
    }

    /// A join for a room this server does not have is an error, not a silent
    /// alias onto the one document it does have.
    #[tokio::test]
    async fn an_unknown_room_is_refused_rather_than_aliased_onto_the_document() {
        let served = dashboard().await;
        let mut client = connect(served.dashboard.addr()).await;

        send(&mut client, &join_request("some-other-room")).await;

        match next_message(&mut client).await {
            ProtocolMessage::JoinError {
                room_id,
                code,
                app_code,
                ..
            } => {
                assert_eq!(room_id, "some-other-room");
                assert_eq!(code, JoinErrorCode::AppError);
                assert_eq!(app_code.as_deref(), Some("unknown_room"));
            }
            other => panic!("expected a JoinError, got {other:?}"),
        }

        // The refusal did not admit the client to the real room either.
        let batch_id = BatchId([2; 8]);
        send(
            &mut client,
            &ProtocolMessage::DocUpdate {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                updates: vec![independent_update("smuggled")],
                batch_id,
            },
        )
        .await;
        assert_eq!(
            next_message(&mut client).await,
            ProtocolMessage::Ack {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                ref_id: batch_id,
                status: UpdateStatusCode::PermissionDenied,
            }
        );
    }

    /// A split update reassembles in index order and is acked once, on the
    /// batch id, when the last fragment lands.
    #[tokio::test]
    async fn a_fragmented_update_reassembles_and_is_acked_once() {
        let served = dashboard().await;
        let addr = served.dashboard.addr();
        let mut alice = connect(addr).await;
        let mut bob = connect(addr).await;
        join(&mut alice).await;
        let seed = join(&mut bob).await;

        let update = independent_update("fragmented");
        let split = update.len() / 2;
        let batch_id = BatchId([7; 8]);
        send(
            &mut alice,
            &ProtocolMessage::DocUpdateFragmentHeader {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                batch_id,
                fragment_count: 2,
                total_size_bytes: u64::try_from(update.len()).expect("a sane update size"),
            },
        )
        .await;
        for (index, chunk) in [&update[..split], &update[split..]].into_iter().enumerate() {
            send(
                &mut alice,
                &ProtocolMessage::DocUpdateFragment {
                    crdt: CrdtType::Loro,
                    room_id: ROOM_ID.to_owned(),
                    batch_id,
                    index: u64::try_from(index).expect("a sane fragment index"),
                    fragment: chunk.to_vec(),
                },
            )
            .await;
        }

        // Exactly one Ack, and only after the batch completed: the first frame
        // back is the Ack, not an early answer to the header or fragment 0.
        assert_eq!(
            next_message(&mut alice).await,
            ProtocolMessage::Ack {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                ref_id: batch_id,
                status: UpdateStatusCode::Ok,
            }
        );

        let ProtocolMessage::DocUpdate { updates, .. } = next_message(&mut bob).await else {
            panic!("expected the reassembled update to relay");
        };
        let mirror = LoroDoc::new();
        mirror.import(&seed).expect("import the seed");
        for update in &updates {
            mirror.import(update).expect("import the relayed delta");
        }
        assert_eq!(title_of(&mirror).as_deref(), Some("fragmented"));
    }

    /// A header whose fragment count is past the bound is refused before a slot
    /// vector is sized from it: the count is the client's claim on this
    /// process's memory.
    #[tokio::test]
    async fn a_fragment_header_past_the_bound_is_refused() {
        let served = dashboard().await;
        let mut client = connect(served.dashboard.addr()).await;
        join(&mut client).await;

        let batch_id = BatchId([3; 8]);
        send(
            &mut client,
            &ProtocolMessage::DocUpdateFragmentHeader {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                batch_id,
                fragment_count: MAX_FRAGMENTS + 1,
                total_size_bytes: 64,
            },
        )
        .await;

        assert_eq!(
            next_message(&mut client).await,
            ProtocolMessage::Ack {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                ref_id: batch_id,
                status: UpdateStatusCode::InvalidUpdate,
            }
        );
    }

    /// Keepalive is text on both sides. A server that answers in binary, or not
    /// at all, gets a full client reconnect every 40 seconds.
    #[tokio::test]
    async fn a_text_ping_is_answered_with_a_text_pong() {
        let served = dashboard().await;
        let mut client = connect(served.dashboard.addr()).await;

        client
            .send(WsMessage::Text("ping".into()))
            .await
            .expect("send the keepalive");

        match next(&mut client).await {
            WsMessage::Text(text) => assert_eq!(text.as_str(), "pong"),
            other => panic!("keepalive must answer with a text frame, got {other:?}"),
        }
    }

    /// The client sends `JoinError` to report that it could not decode something
    /// this server said. It is undocumented but real, and it must not take the
    /// connection down with it.
    #[tokio::test]
    async fn an_inbound_join_error_does_not_end_the_connection() {
        let served = dashboard().await;
        let mut client = connect(served.dashboard.addr()).await;

        send(
            &mut client,
            &ProtocolMessage::JoinError {
                crdt: CrdtType::Loro,
                room_id: ROOM_ID.to_owned(),
                code: JoinErrorCode::Unknown,
                message: "Invalid version format received".to_owned(),
                receiver_version: None,
                app_code: None,
            },
        )
        .await;

        // Still serving: the join that follows is answered normally.
        send(&mut client, &join_request(ROOM_ID)).await;
        let frame = next_binary(&mut client).await;
        assert_eq!(frame[TYPE_AT], 0x01, "JoinResponseOk");
    }
}
