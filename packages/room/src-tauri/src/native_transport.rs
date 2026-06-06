use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{Emitter, Manager, WebviewWindow};
use tokio::sync::mpsc;
use wtransport::error::StreamReadExactError;
use wtransport::tls::Sha256Digest;
use wtransport::{ClientConfig, Endpoint};

const EVENT_NAME: &str = "room://native-transport-event";

const TAG_BINARY: u8 = b'B';
const MAX_FRAME_LEN: u32 = 64 * 1024 * 1024;

#[derive(Default)]
pub struct NativeTransportManager {
    sessions: Mutex<HashMap<u64, NativeTransportSession>>,
}

struct NativeTransportSession {
    tx: mpsc::UnboundedSender<NativeTransportCommand>,
}

enum NativeTransportCommand {
    Loro(Vec<u8>),
    Datagram(Vec<u8>),
    Close,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum NativeTransportEvent {
    Open { id: u64 },
    Frame { id: u64, tag: u8, payload: Vec<u8> },
    Datagram { id: u64, payload: Vec<u8> },
    Closed { id: u64 },
    Error { id: u64, message: String },
}

struct InboundFrame {
    tag: u8,
    payload: Vec<u8>,
}

#[tauri::command]
pub fn native_transport_connect(
    window: WebviewWindow,
    manager: tauri::State<'_, NativeTransportManager>,
    id: u64,
    wt_url: String,
    cert_sha256_hex: String,
) -> Result<(), String> {
    let (tx, rx) = mpsc::unbounded_channel();
    {
        let mut sessions = manager
            .sessions
            .lock()
            .map_err(|_| "native transport session table is poisoned".to_string())?;
        if let Some(previous) = sessions.insert(id, NativeTransportSession { tx }) {
            let _ = previous.tx.send(NativeTransportCommand::Close);
        }
    }

    tauri::async_runtime::spawn(run_session(window, id, wt_url, cert_sha256_hex, rx));
    Ok(())
}

#[tauri::command]
pub fn native_transport_send_loro(
    manager: tauri::State<'_, NativeTransportManager>,
    id: u64,
    bytes: Vec<u8>,
) -> Result<(), String> {
    send_command(&manager, id, NativeTransportCommand::Loro(bytes))
}

#[tauri::command]
pub fn native_transport_send_datagram(
    manager: tauri::State<'_, NativeTransportManager>,
    id: u64,
    bytes: Vec<u8>,
) -> Result<(), String> {
    send_command(&manager, id, NativeTransportCommand::Datagram(bytes))
}

#[tauri::command]
pub fn native_transport_close(
    manager: tauri::State<'_, NativeTransportManager>,
    id: u64,
) -> Result<(), String> {
    let previous = manager
        .sessions
        .lock()
        .map_err(|_| "native transport session table is poisoned".to_string())?
        .remove(&id);
    if let Some(session) = previous {
        let _ = session.tx.send(NativeTransportCommand::Close);
    }
    Ok(())
}

fn send_command(
    manager: &NativeTransportManager,
    id: u64,
    command: NativeTransportCommand,
) -> Result<(), String> {
    let sessions = manager
        .sessions
        .lock()
        .map_err(|_| "native transport session table is poisoned".to_string())?;
    let session = sessions
        .get(&id)
        .ok_or_else(|| format!("native transport session {id} is not open"))?;
    session
        .tx
        .send(command)
        .map_err(|_| format!("native transport session {id} is closed"))
}

async fn run_session(
    window: WebviewWindow,
    id: u64,
    wt_url: String,
    cert_sha256_hex: String,
    mut rx: mpsc::UnboundedReceiver<NativeTransportCommand>,
) {
    if let Err(err) = run_session_inner(&window, id, &wt_url, &cert_sha256_hex, &mut rx).await {
        emit(&window, NativeTransportEvent::Error { id, message: err });
    }
    emit(&window, NativeTransportEvent::Closed { id });
    if let Some(manager) = window.try_state::<NativeTransportManager>() {
        if let Ok(mut sessions) = manager.sessions.lock() {
            sessions.remove(&id);
        }
    }
}

async fn run_session_inner(
    window: &WebviewWindow,
    id: u64,
    wt_url: &str,
    cert_sha256_hex: &str,
    rx: &mut mpsc::UnboundedReceiver<NativeTransportCommand>,
) -> Result<(), String> {
    let digest = parse_cert_hash(cert_sha256_hex)?;
    let config = ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes([digest])
        .build();
    let endpoint = Endpoint::client(config).map_err(|err| format!("build endpoint: {err}"))?;
    let connection = endpoint
        .connect(wt_url)
        .await
        .map_err(|err| format!("connect webtransport: {err}"))?;
    let (mut sender, mut receiver) = connection
        .open_bi()
        .await
        .map_err(|err| format!("open sync stream: {err}"))?
        .await
        .map_err(|err| format!("finish sync stream open: {err}"))?;

    emit(window, NativeTransportEvent::Open { id });

    let datagram_connection = connection.clone();
    let datagram_window = window.clone();
    let datagram_task = tauri::async_runtime::spawn(async move {
        loop {
            match datagram_connection.receive_datagram().await {
                Ok(datagram) => emit(
                    &datagram_window,
                    NativeTransportEvent::Datagram {
                        id,
                        payload: datagram.payload().to_vec(),
                    },
                ),
                Err(err) => {
                    emit(
                        &datagram_window,
                        NativeTransportEvent::Error {
                            id,
                            message: format!("receive datagram: {err}"),
                        },
                    );
                    break;
                }
            }
        }
    });

    loop {
        tokio::select! {
            command = rx.recv() => {
                match command {
                    Some(NativeTransportCommand::Loro(bytes)) => {
                        send_frame(&mut sender, TAG_BINARY, &bytes).await?;
                    }
                    Some(NativeTransportCommand::Datagram(bytes)) => {
                        connection
                            .send_datagram(bytes)
                            .map_err(|err| format!("send datagram: {err}"))?;
                    }
                    Some(NativeTransportCommand::Close) | None => {
                        connection.close(wtransport::VarInt::from_u32(0), b"client closing");
                        break;
                    }
                }
            }
            frame = read_frame(&mut receiver) => {
                match frame? {
                    Some(frame) => emit(
                        window,
                        NativeTransportEvent::Frame {
                            id,
                            tag: frame.tag,
                            payload: frame.payload,
                        },
                    ),
                    None => break,
                }
            }
        }
    }

    datagram_task.abort();
    endpoint.close(wtransport::VarInt::from_u32(0), b"client closing");
    Ok(())
}

fn emit(window: &WebviewWindow, event: NativeTransportEvent) {
    if let Err(err) = window.emit(EVENT_NAME, event) {
        eprintln!("room: native transport event emit failed: {err}");
    }
}

fn parse_cert_hash(hex: &str) -> Result<Sha256Digest, String> {
    let mut bytes = [0u8; 32];
    if hex.len() != bytes.len() * 2 {
        return Err("cert hash must be 32 bytes of lowercase hex".to_string());
    }
    for (idx, slot) in bytes.iter_mut().enumerate() {
        let start = idx * 2;
        *slot = u8::from_str_radix(&hex[start..start + 2], 16)
            .map_err(|_| "cert hash contains non-hex bytes".to_string())?;
    }
    Ok(Sha256Digest::new(bytes))
}

async fn send_frame(
    stream: &mut wtransport::SendStream,
    tag: u8,
    payload: &[u8],
) -> Result<(), String> {
    let len = u32::try_from(payload.len()).map_err(|_| "frame too large".to_string())?;
    let mut header = [0u8; 5];
    header[0] = tag;
    header[1..].copy_from_slice(&len.to_be_bytes());
    stream
        .write_all(&header)
        .await
        .map_err(|err| format!("write sync frame header: {err}"))?;
    stream
        .write_all(payload)
        .await
        .map_err(|err| format!("write sync frame payload: {err}"))?;
    Ok(())
}

async fn read_frame(stream: &mut wtransport::RecvStream) -> Result<Option<InboundFrame>, String> {
    let mut header = [0u8; 5];
    match stream.read_exact(&mut header).await {
        Ok(_) => {}
        Err(StreamReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(err) => return Err(format!("read sync frame header: {err}")),
    }
    let tag = header[0];
    let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    if len > MAX_FRAME_LEN {
        return Err(format!("frame length {len} exceeds limit"));
    }
    let mut payload = vec![0u8; len as usize];
    if len > 0 {
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|err| format!("read sync frame payload: {err}"))?;
    }
    Ok(Some(InboundFrame { tag, payload }))
}
