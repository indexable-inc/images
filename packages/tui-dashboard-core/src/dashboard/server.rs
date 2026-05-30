//! The HTTP surface: one page plus an SSE endpoint, served from a [`Hub`].
//!
//! [`serve_hub`] binds the listener, starts the server task, and hands back a
//! [`Dashboard`] handle plus a shutdown receiver the caller threads into its own
//! frame-source tasks (the in-process poller, or the aggregator's socket
//! readers). One owner for the router means the in-process dashboard and the
//! standalone aggregator render through exactly the same page and stream.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use futures::{Stream, StreamExt as _};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use super::hub::Hub;
use crate::{Error, Result};

/// The single page the dashboard serves; it connects back over `/events`.
const DASHBOARD_HTML: &str = include_str!("dashboard.html");

/// A running dashboard server. Dropping it, or calling [`Dashboard::stop`],
/// shuts the HTTP server and every attached frame-source task down.
pub struct Dashboard {
    addr: SocketAddr,
    shutdown: Option<watch::Sender<bool>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Dashboard {
    /// The address the server bound to (the resolved port when `:0` was given).
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The URL to open in a browser.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }

    /// Attach a frame-source task whose lifetime is tied to this dashboard, so
    /// [`stop`](Self::stop) and `Drop` wind it down with the server.
    pub fn push_task(&mut self, task: JoinHandle<()>) {
        self.tasks.push(task);
    }

    /// Stop the server and every attached task, waiting for them to wind down.
    /// Idempotent.
    ///
    /// The tasks are aborted, not just signalled: an open Server-Sent-Events
    /// stream never ends on its own, so `axum`'s graceful shutdown would block
    /// forever while any browser is connected. Aborting drops those streams and
    /// returns promptly.
    pub async fn stop(&mut self) {
        let Some(shutdown) = self.shutdown.take() else {
            return;
        };
        let _ = shutdown.send(true);
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Dashboard {
    /// Best-effort, non-blocking teardown for a dropped handle that was never
    /// `stop`ped: signal shutdown and abort the tasks. They wind down on the
    /// runtime; `Drop` cannot await, so it does not join them.
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(true);
        }
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

/// Bind `addr`, start the HTTP server for `hub`, and return the handle plus a
/// shutdown receiver.
///
/// The server task runs on `runtime`, which must outlive the dashboard (the
/// manager's runtime for the in-process `tui::serve`, the process runtime for
/// the aggregator). The caller spawns its frame-source tasks on the same runtime
/// against the returned receiver and attaches them with
/// [`Dashboard::push_task`], so one shutdown signal stops the whole dashboard.
///
/// # Errors
///
/// Returns [`Error::Dashboard`] when `addr` cannot be bound or its resolved
/// local address cannot be read.
pub async fn serve_hub(
    hub: Arc<Hub>,
    addr: SocketAddr,
    runtime: &tokio::runtime::Handle,
) -> Result<(Dashboard, watch::Receiver<bool>)> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| Error::Dashboard {
            message: format!("bind {addr}: {source}"),
        })?;
    let bound = listener.local_addr().map_err(|source| Error::Dashboard {
        message: format!("local_addr: {source}"),
    })?;

    let (shutdown, stop_rx) = watch::channel(false);

    let app = Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/events", get(events))
        .with_state(hub);

    let http = {
        let mut rx = shutdown.subscribe();
        runtime.spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                let _ = rx.wait_for(|stop| *stop).await;
            });
            let _ = server.await;
        })
    };

    let dashboard = Dashboard {
        addr: bound,
        shutdown: Some(shutdown),
        tasks: vec![http],
    };
    Ok((dashboard, stop_rx))
}

async fn index() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn events(State(hub): State<Arc<Hub>>) -> impl IntoResponse {
    // Loro imports are idempotent, so an overlapping snapshot/update is harmless.
    let (snapshot, rx) = hub.subscribe();

    let first = futures::stream::once(async move {
        Ok::<_, Infallible>(
            Event::default()
                .event("snapshot")
                .data(super::b64(&snapshot)),
        )
    });

    let tail = BroadcastStream::new(rx).map(move |item| {
        let event = match item {
            Ok(encoded) => Event::default().event("update").data(encoded.as_ref()),
            Err(BroadcastStreamRecvError::Lagged(_)) => {
                Event::default().event("snapshot").data(hub.snapshot_b64())
            }
        };
        Ok::<_, Infallible>(event)
    });

    let stream: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(first.chain(tail));
    Sse::new(stream).keep_alive(KeepAlive::default())
}
