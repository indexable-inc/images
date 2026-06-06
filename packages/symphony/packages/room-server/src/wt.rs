// WebTransport fan-out.
//
// Each peer connects with `new WebTransport("https://host:wt_port",
// { serverCertificateHashes: [...] })` and opens a single
// bidirectional sync stream. Two interleaved streams ride that one
// reliable stream and one shared QUIC datagram channel:
//
//   - tag 'J' length-prefixed JSON text frames carry thread/message
//     deltas, the bootstrap, and periodic pings.
//   - tag 'B' length-prefixed binary frames carry opaque Loro CRDT
//     updates for ephemeral presence; the server imports and
//     persists them before relaying to peers.
//   - QUIC datagrams carry Opus audio packets prefixed by an 8-byte
//     big-endian peer id. The server stamps the peer id on inbound
//     audio and rebroadcasts to every other peer; receivers drop
//     frames whose stamp matches their own.
//
// Per-thread subscriptions are not yet implemented; the total event
// volume is small and the broadcast is cheap.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use bytes::{BufMut, BytesMut};
use loro::ExportMode;
use rand::RngCore;
use serde::Serialize;
use tokio::time::interval;
use wtransport::endpoint::{IncomingSession, endpoint_side::Server};
use wtransport::error::StreamReadExactError;
use wtransport::{Endpoint, ServerConfig};

use crate::{codex_bridge::Delta, db::Thread, state::AppState};

const TAG_JSON: u8 = b'J';
const TAG_BINARY: u8 = b'B';
const TAG_PING: u8 = b'P';

const MAX_FRAME_LEN: u32 = 64 * 1024 * 1024;

const AUDIO_PEER_ID_LEN: usize = 8;

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ServerEvent {
    Bootstrap {
        threads: Vec<Thread>,
    },
    ThreadUpsert {
        thread: Thread,
    },
    MessageAppend {
        thread_id: String,
        message: crate::db::Message,
    },
    MessageUpdate {
        thread_id: String,
        message: crate::db::Message,
    },
    ThreadArchive {
        thread_id: String,
    },
    Ping,
}

/// Take ownership of a configured wtransport `ServerConfig` and run
/// the accept loop until `shutdown` resolves. Each accepted session
/// gets its own task; failures inside a session are logged and do
/// not bring the listener down.
pub async fn serve(
    config: ServerConfig,
    state: AppState,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let endpoint: Endpoint<Server> =
        Endpoint::server(config).context("build wtransport server endpoint")?;
    let local_addr = endpoint
        .local_addr()
        .context("wtransport endpoint has no local addr")?;
    eprintln!("room-server: wtransport listening on {local_addr}");

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => {
                eprintln!("room-server: wtransport listener shutting down");
                endpoint.close(wtransport::VarInt::from_u32(0), b"server shutting down");
                return Ok(());
            }
            incoming = endpoint.accept() => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_session(incoming, state).await {
                        eprintln!("room-server: wtransport session error: {err:#}");
                    }
                });
            }
        }
    }
}

async fn handle_session(incoming: IncomingSession, state: AppState) -> Result<()> {
    let session_request = incoming.await.context("await session request")?;
    let connection = session_request
        .accept()
        .await
        .context("accept session request")?;

    let mut peer_id_bytes = [0u8; AUDIO_PEER_ID_LEN];
    rand::rng().fill_bytes(&mut peer_id_bytes);

    let audio_state = state.clone();
    let audio_peer_id = peer_id_bytes;
    let connection_for_audio = connection.clone();
    let audio_task = tokio::spawn(async move {
        if let Err(err) = audio_fanout(connection_for_audio, audio_state, audio_peer_id).await {
            eprintln!("room-server: audio fanout ended: {err:#}");
        }
    });

    let sync_result = sync_stream(connection, state).await;
    audio_task.abort();
    sync_result
}

async fn sync_stream(connection: wtransport::Connection, state: AppState) -> Result<()> {
    // The client opens one bidi stream and uses it for the
    // lifetime of the session. Wait for that, then mirror the old
    // ws handler's frame loop on it.
    let (mut sender, mut receiver) = connection
        .accept_bi()
        .await
        .context("accept sync bidi stream")?;

    let bootstrap_threads = {
        let db = state.db.lock().await;
        match db.list_threads(&crate::db::ThreadFilter {
            limit: 50,
            ..Default::default()
        }) {
            Ok(threads) => threads,
            Err(err) => {
                eprintln!("room: wt bootstrap failed: {err:#}");
                Vec::new()
            }
        }
    };
    let bootstrap_json = serde_json::to_vec(&ServerEvent::Bootstrap {
        threads: bootstrap_threads,
    })
    .context("encode bootstrap")?;
    send_frame(&mut sender, TAG_JSON, &bootstrap_json).await?;

    let snapshot = {
        let doc = state.loro_doc.lock().await;
        doc.export(ExportMode::Snapshot)
    };
    match snapshot {
        Ok(bytes) if !bytes.is_empty() => {
            send_frame(&mut sender, TAG_BINARY, &bytes).await?;
        }
        Ok(_) => {}
        Err(err) => {
            eprintln!("room: failed to export loro snapshot on connect: {err:?}");
        }
    }

    let mut rx = state.broadcast.subscribe();
    let mut loro_rx = state.loro_broadcast.subscribe();
    let mut keepalive = interval(Duration::from_secs(20));
    keepalive.tick().await; // discard the first immediate tick

    loop {
        tokio::select! {
            biased;
            frame = read_frame(&mut receiver) => {
                let frame = match frame {
                    Ok(Some(frame)) => frame,
                    Ok(None) => break,
                    Err(err) => {
                        eprintln!("room: malformed wt sync frame: {err:#}");
                        break;
                    }
                };
                if frame.tag == TAG_BINARY {
                    handle_inbound_loro(&state, frame.payload).await;
                }
            }
            delta = rx.recv() => {
                match delta {
                    Err(_) => break,
                    Ok(delta) => {
                        let event = match delta {
                            Delta::ThreadUpsert { thread } => ServerEvent::ThreadUpsert { thread },
                            Delta::MessageAppend { thread_id, message } => {
                                ServerEvent::MessageAppend { thread_id, message }
                            }
                            Delta::MessageUpdate { thread_id, message } => {
                                ServerEvent::MessageUpdate { thread_id, message }
                            }
                            Delta::ThreadArchive { thread_id } => {
                                ServerEvent::ThreadArchive { thread_id }
                            }
                        };
                        let Ok(text) = serde_json::to_vec(&event) else { continue };
                        if send_frame(&mut sender, TAG_JSON, &text).await.is_err() {
                            break;
                        }
                    }
                }
            }
            frame = loro_rx.recv() => {
                match frame {
                    Err(_) => break,
                    Ok(bytes) => {
                        if send_frame(&mut sender, TAG_BINARY, &bytes).await.is_err() {
                            break;
                        }
                    }
                }
            }
            _ = keepalive.tick() => {
                let Ok(text) = serde_json::to_vec(&ServerEvent::Ping) else { continue };
                if send_frame(&mut sender, TAG_PING, &text).await.is_err() {
                    break;
                }
            }
        }
    }

    Ok(())
}

async fn handle_inbound_loro(state: &AppState, bytes: Vec<u8>) {
    let mut import_ok = false;
    {
        let doc = state.loro_doc.lock().await;
        match doc.import(&bytes) {
            Ok(_) => import_ok = true,
            Err(err) => {
                eprintln!(
                    "room: failed to import loro frame ({} bytes): {err:?}",
                    bytes.len()
                );
            }
        }
    }
    if import_ok {
        let db = state.db.clone();
        let to_persist = bytes.clone();
        let ts = now_ms();
        tokio::spawn(async move {
            let db = db.lock().await;
            if let Err(err) = db.append_loro_update(ts, &to_persist) {
                eprintln!("room: failed to persist loro frame: {err:?}");
            }
        });

        let doc = state.loro_doc.clone();
        let db = state.db.clone();
        let mirror = state.annotation_mirror.clone();
        tokio::spawn(async move {
            let doc = doc.lock().await;
            let mut db = db.lock().await;
            let mut mirror = mirror.lock().await;
            if let Err(err) = mirror.sync(&doc, &mut db) {
                eprintln!("room: failed to mirror annotations: {err:#}");
            }
        });
    }
    let _ = state.loro_broadcast.send(bytes);
}

async fn audio_fanout(
    connection: wtransport::Connection,
    state: AppState,
    peer_id: [u8; AUDIO_PEER_ID_LEN],
) -> Result<()> {
    let mut audio_rx = state.audio_broadcast.subscribe();

    loop {
        tokio::select! {
            incoming = connection.receive_datagram() => {
                let dgram = match incoming {
                    Ok(d) => d,
                    Err(err) => return Err(anyhow!("receive datagram: {err}")),
                };
                let payload = dgram.payload();
                if payload.is_empty() { continue; }
                // The client sends raw Opus packets without the peer
                // id prefix; the server stamps its own assigned id so
                // peers can identify the source and filter echoes.
                let mut framed = BytesMut::with_capacity(AUDIO_PEER_ID_LEN + payload.len());
                framed.extend_from_slice(&peer_id);
                framed.extend_from_slice(&payload);
                let _ = state.audio_broadcast.send(framed.freeze());
            }
            outbound = audio_rx.recv() => {
                let frame = match outbound {
                    Ok(b) => b,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if frame.len() < AUDIO_PEER_ID_LEN { continue; }
                if frame[..AUDIO_PEER_ID_LEN] == peer_id { continue; }
                if let Err(err) = connection.send_datagram(frame) {
                    // Datagram write failures are normal under
                    // congestion (oversize, queue full). Drop the
                    // frame and keep going; audio is unreliable by
                    // contract.
                    eprintln!("room: drop audio datagram: {err}");
                }
            }
        }
    }
    Ok(())
}

struct InboundFrame {
    tag: u8,
    payload: Vec<u8>,
}

async fn send_frame(stream: &mut wtransport::SendStream, tag: u8, payload: &[u8]) -> Result<()> {
    let len = u32::try_from(payload.len()).context("frame too large for u32 length")?;
    let mut header = BytesMut::with_capacity(5);
    header.put_u8(tag);
    header.put_u32(len);
    stream
        .write_all(&header)
        .await
        .context("write sync frame header")?;
    stream
        .write_all(payload)
        .await
        .context("write sync frame payload")?;
    Ok(())
}

async fn read_frame(stream: &mut wtransport::RecvStream) -> Result<Option<InboundFrame>> {
    let mut header = [0u8; 5];
    match stream.read_exact(&mut header).await {
        Ok(_) => {}
        Err(StreamReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(err) => return Err(anyhow!("read sync frame header: {err}")),
    }
    let tag = header[0];
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    if len > MAX_FRAME_LEN {
        return Err(anyhow!("frame length {len} exceeds limit"));
    }
    let mut payload = vec![0u8; len as usize];
    if len > 0 {
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|err| anyhow!("read sync frame payload: {err}"))?;
    }
    Ok(Some(InboundFrame { tag, payload }))
}
