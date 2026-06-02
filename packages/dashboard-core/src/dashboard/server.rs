//! The HTTP surface: one page, an SSE endpoint, and the recordings routes,
//! served from a [`Hub`] (and an optional [`RecordingStore`]).
//!
//! [`serve_hub`] binds the listener, starts the server task, and hands back a
//! [`Dashboard`] handle plus a shutdown receiver the caller threads into its own
//! frame-source tasks (the in-process poller, or the aggregator's socket
//! readers). One owner for the router means the in-process dashboard and the
//! standalone aggregator render through exactly the same page and stream. When a
//! store is supplied, `/recordings` lists saved sessions and `/recording/<id>`
//! serves one snapshot for replay; without one, those routes report no
//! recordings, so the in-process dashboard works unchanged.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use futures::{Stream, StreamExt as _};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use super::hub::Hub;
use super::recordings::RecordingStore;
use crate::{Error, Result};

/// The single page the dashboard serves; it connects back over `/events`.
const DASHBOARD_HTML: &str = include_str!("dashboard.html");

/// Shared router state: the live document and, optionally, the on-disk
/// recordings. Cloning is cheap: both fields are `Arc`s.
#[derive(Clone)]
struct AppState {
    hub: Arc<Hub>,
    recordings: Option<Arc<RecordingStore>>,
}

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
/// `recordings` enables the replay routes: pass the aggregator's
/// [`RecordingStore`] to serve saved sessions, or `None` (the in-process
/// dashboard) to leave the routes reporting an empty list.
///
/// # Errors
///
/// Returns [`Error::Dashboard`] when `addr` cannot be bound or its resolved
/// local address cannot be read.
pub async fn serve_hub(
    hub: Arc<Hub>,
    addr: SocketAddr,
    recordings: Option<Arc<RecordingStore>>,
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
        .route("/recordings", get(list_recordings))
        .route("/recording/{id}", get(get_recording))
        .with_state(AppState { hub, recordings });

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

/// List saved recordings as JSON, newest first. Without a store (the in-process
/// dashboard) the list is empty, so the frontend simply offers no recordings.
async fn list_recordings(State(state): State<AppState>) -> impl IntoResponse {
    let recordings = state.recordings.map(|store| store.list()).unwrap_or_default();
    Json(recordings)
}

/// Serve one recording's snapshot bytes for the frontend to import into a
/// detached document and replay. The id is validated by the store, so a bad or
/// traversing id is a 404 rather than a path escape.
async fn get_recording(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    state.recordings.and_then(|store| store.load(&id)).map_or_else(
        || (StatusCode::NOT_FOUND, "no such recording").into_response(),
        |bytes| ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
    )
}

async fn events(State(state): State<AppState>) -> impl IntoResponse {
    let hub = state.hub;
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
