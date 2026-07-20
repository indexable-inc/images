//! Websocket endpoints: the per-session terminal channel and the session-list
//! events channel.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt as _, StreamExt as _};
use tokio::sync::broadcast;

use crate::proto::{ClientMsg, ServerMsg};
use crate::session::{Session, SessionManager};

/// Serialize a message for the wire.
fn frame(msg: &ServerMsg) -> Message {
    Message::Text(
        serde_json::to_string(msg)
            .expect("wire types serialize (pinned by proto tests)")
            .into(),
    )
}

/// Drive one terminal client: forward the session's event stream out and
/// apply the client's input/resize/refresh messages.
pub async fn terminal_client(session: Arc<Session>, socket: WebSocket) {
    let conn = session.next_conn();
    // Subscribe before requesting the full frame so it cannot be missed.
    let mut events = session.subscribe();
    let (mut sink, mut stream) = socket.split();

    // Joining state: identity, seat, opened doc, then a full grid.
    let hello = ServerMsg::Hello {
        conn: conn.to_string(),
        session: session.meta(),
    };
    for msg in [&hello, &session.driver_msg(), &session.open_msg()] {
        if sink.send(frame(msg)).await.is_err() {
            return;
        }
    }
    session.request_full_frame();

    // Outbound pump. A slow client that lags off the broadcast ring is
    // resynced with a fresh full frame instead of the dropped ones.
    let pump_session = Arc::clone(&session);
    let mut pump = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(msg) => {
                    if sink.send(frame(msg.as_ref())).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    pump_session.request_full_frame();
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });

    // Inbound loop on this task; ends on disconnect or pump failure.
    loop {
        tokio::select! {
            _ = &mut pump => break,
            incoming = stream.next() => {
                let Some(Ok(message)) = incoming else { break };
                let Message::Text(text) = message else { continue };
                match serde_json::from_str::<ClientMsg>(&text) {
                    Ok(ClientMsg::Input { data }) => session.write_input(conn, &data).await,
                    Ok(ClientMsg::Resize { cols, rows }) => session.resize(conn, rows, cols).await,
                    Ok(ClientMsg::Refresh) => session.request_full_frame(),
                    Ok(ClientMsg::CloseDoc) => session.close_doc(),
                    Err(error) => {
                        tracing::debug!(%error, "ignoring malformed client message");
                    }
                }
            }
        }
    }

    pump.abort();
    session.release_driver(conn);
}

/// Drive one events client: push the session list on connect and on change.
pub async fn events_client(manager: Arc<SessionManager>, mut socket: WebSocket) {
    let mut list = manager.watch_list();
    loop {
        let sessions = list.borrow_and_update().clone();
        let msg = ServerMsg::Sessions { sessions };
        if socket.send(frame(&msg)).await.is_err() {
            return;
        }
        tokio::select! {
            changed = list.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            // Drain (and thereby notice the close of) the client side.
            incoming = socket.recv() => {
                if incoming.is_none() {
                    return;
                }
            }
        }
    }
}
