//! A read-only web dashboard for the live terminals of one [`TuiManager`].
//!
//! [`serve`] starts an HTTP server on the manager's own tokio runtime. A poll
//! task samples every tracked terminal's viewport, writes it into a single
//! [`loro::LoroDoc`], and streams the resulting CRDT updates to every connected
//! browser over Server-Sent Events. The browser holds its own `loro-crdt`
//! document, imports the bytes, and paints a grid of every terminal.
//!
//! Loro is only the view-sync layer: the PTYs and their authoritative state
//! stay in this process. Browsers never write back, so the doc has a single
//! editor and conflict resolution never actually runs; the CRDT buys cheap
//! incremental text diffs and a late joiner catching up from one snapshot.
//!
//! ```no_run
//! use std::sync::Arc;
//! use std::time::Duration;
//! use tui::TuiManager;
//!
//! # async fn run() -> Result<(), tui::Error> {
//! let manager = Arc::new(TuiManager::new());
//! let dashboard = tui::serve(&manager, "127.0.0.1:0".parse().unwrap(), Duration::from_millis(100)).await?;
//! println!("open {}", dashboard.url());
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures::{Stream, StreamExt as _};
use loro::{ExportMode, LoroDoc, LoroMap, LoroText, VersionVector};
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;

use crate::{Error, Result, TuiManager};

/// The single page the dashboard serves; it connects back over `/events`.
const DASHBOARD_HTML: &str = include_str!("dashboard.html");

/// How many CRDT updates a slow SSE client may fall behind before it is fed a
/// fresh snapshot instead. Bounds memory per connection.
const BROADCAST_CAPACITY: usize = 256;

/// One terminal's rendered state, lifted out of the manager before it crosses
/// into the Loro document under lock. Named rather than a tuple so the poll
/// boundary stays legible.
struct Frame {
    id: String,
    command: String,
    args: String,
    rows: u16,
    cols: u16,
    alive: bool,
    screen: String,
}

/// The Loro handles backing one terminal card, cached across polls so the loop
/// does not re-resolve containers by key every tick. Valid until the key is
/// deleted from the root map.
struct Slot {
    meta: LoroMap,
    screen: LoroText,
    alive: bool,
}

/// The shared document plus the per-terminal handles and the version already
/// streamed to live clients.
struct DocState {
    doc: LoroDoc,
    root: LoroMap,
    terminals: HashMap<String, Slot>,
    streamed: VersionVector,
}

impl DocState {
    fn new() -> Self {
        let doc = LoroDoc::new();
        let root = doc.get_map("terminals");
        let streamed = doc.oplog_vv();
        Self {
            doc,
            root,
            terminals: HashMap::new(),
            streamed,
        }
    }

    /// Reconcile the document with `frames`, returning the CRDT delta since the
    /// last broadcast when anything actually changed.
    fn sync(&mut self, frames: &[Frame]) -> Result<Option<Vec<u8>>> {
        for frame in frames {
            if !self.terminals.contains_key(&frame.id) {
                let meta = self
                    .root
                    .insert_container(frame.id.as_str(), LoroMap::new())
                    .map_err(loro_err)?;
                meta.insert("command", frame.command.as_str())
                    .map_err(loro_err)?;
                meta.insert("args", frame.args.as_str()).map_err(loro_err)?;
                meta.insert("rows", i64::from(frame.rows))
                    .map_err(loro_err)?;
                meta.insert("cols", i64::from(frame.cols))
                    .map_err(loro_err)?;
                meta.insert("alive", frame.alive).map_err(loro_err)?;
                let screen = meta
                    .insert_container("screen", LoroText::new())
                    .map_err(loro_err)?;
                self.terminals.insert(
                    frame.id.clone(),
                    Slot {
                        meta,
                        screen,
                        alive: frame.alive,
                    },
                );
            }

            let slot = self
                .terminals
                .get_mut(&frame.id)
                .expect("slot inserted above");
            if slot.alive != frame.alive {
                slot.meta.insert("alive", frame.alive).map_err(loro_err)?;
                slot.alive = frame.alive;
            }
            if slot.screen.to_string() != frame.screen {
                slot.screen
                    .update(&frame.screen, loro::UpdateOptions::default())
                    .map_err(|source| Error::Dashboard {
                        message: format!("text update: {source}"),
                    })?;
            }
        }

        let live: std::collections::HashSet<&str> =
            frames.iter().map(|frame| frame.id.as_str()).collect();
        let dead: Vec<String> = self
            .terminals
            .keys()
            .filter(|id| !live.contains(id.as_str()))
            .cloned()
            .collect();
        for id in dead {
            self.root.delete(&id).map_err(loro_err)?;
            self.terminals.remove(&id);
        }

        self.doc.commit();
        let current = self.doc.oplog_vv();
        if current == self.streamed {
            return Ok(None);
        }
        let delta = self
            .doc
            .export(ExportMode::updates(&self.streamed))
            .map_err(loro_err)?;
        self.streamed = current;
        Ok(Some(delta))
    }

    /// A full snapshot of the current document, for a newly-connected client or
    /// one that fell too far behind the update stream.
    fn snapshot(&self) -> Result<Vec<u8>> {
        self.doc.export(ExportMode::Snapshot).map_err(loro_err)
    }
}

fn loro_err(source: impl std::fmt::Display) -> Error {
    Error::Dashboard {
        message: source.to_string(),
    }
}

/// Owns the shared document and fans CRDT updates out to SSE subscribers.
struct Hub {
    state: Mutex<DocState>,
    updates: broadcast::Sender<Arc<str>>,
}

impl Hub {
    fn new() -> Arc<Self> {
        let (updates, _) = broadcast::channel(BROADCAST_CAPACITY);
        Arc::new(Self {
            state: Mutex::new(DocState::new()),
            updates,
        })
    }
}

/// A running dashboard. Dropping it, or calling [`Dashboard::stop`], shuts the
/// HTTP server and poll loop down.
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

    /// Stop the server and poll loop, waiting for the tasks to wind down.
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

/// Serve a read-only dashboard of `manager`'s live terminals on `addr`.
///
/// Pass `127.0.0.1:0` to bind an ephemeral port and read it back from
/// [`Dashboard::addr`]. `poll` is the viewport sampling interval; every tick
/// that changes a terminal produces one CRDT update broadcast to all clients.
///
/// Async because the binding surface is uniformly async: bindings drive it from
/// inside the manager's runtime (so a blocking bind would deadlock), and a
/// pure-Rust caller can `runtime.block_on(serve(..))` when it needs sync.
pub async fn serve(
    manager: &Arc<TuiManager>,
    addr: SocketAddr,
    poll: Duration,
) -> Result<Dashboard> {
    let runtime = manager.runtime();
    let hub = Hub::new();

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| Error::Dashboard {
            message: format!("bind {addr}: {source}"),
        })?;
    let bound = listener.local_addr().map_err(|source| Error::Dashboard {
        message: format!("local_addr: {source}"),
    })?;

    let (shutdown, _) = watch::channel(false);

    let app = Router::new()
        .route("/", get(index))
        .route("/index.html", get(index))
        .route("/events", get(events))
        .with_state(hub.clone());

    let http = {
        let mut rx = shutdown.subscribe();
        runtime.spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                let _ = rx.wait_for(|stop| *stop).await;
            });
            let _ = server.await;
        })
    };

    let poller = {
        let manager = manager.clone();
        let mut rx = shutdown.subscribe();
        runtime.spawn(async move {
            loop {
                poll_once(&manager, &hub).await;
                tokio::select! {
                    () = tokio::time::sleep(poll) => {}
                    _ = rx.wait_for(|stop| *stop) => break,
                }
            }
        })
    };

    Ok(Dashboard {
        addr: bound,
        shutdown: Some(shutdown),
        tasks: vec![http, poller],
    })
}

/// Sample every live terminal, push the changes into the document, and
/// broadcast the resulting delta. A failed tick is dropped on purpose: the
/// dashboard is a best-effort view and the next tick re-renders from scratch.
async fn poll_once(manager: &Arc<TuiManager>, hub: &Hub) {
    let mut frames = Vec::new();
    for instance in manager.list() {
        let Ok(full) = instance.read_full_async().await else {
            continue;
        };
        frames.push(Frame {
            id: instance.id.to_string(),
            command: instance.command.clone(),
            args: instance.args.join(" "),
            rows: instance.rows(),
            cols: instance.cols(),
            alive: instance.is_alive(),
            screen: full.viewport.join("\n"),
        });
    }

    let delta = hub.state.lock().sync(&frames);
    if let Ok(Some(bytes)) = delta {
        let encoded: Arc<str> = Arc::from(BASE64.encode(&bytes).as_str());
        let _ = hub.updates.send(encoded);
    }
}

async fn index() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn events(State(hub): State<Arc<Hub>>) -> impl IntoResponse {
    // Subscribe before snapshotting, both under the doc lock, so the client's
    // snapshot version lines up with the first update it will receive. Loro
    // imports are idempotent, so an overlapping snapshot/update is harmless.
    let (snapshot, rx) = {
        let state = hub.state.lock();
        let rx = hub.updates.subscribe();
        (state.snapshot().unwrap_or_default(), rx)
    };

    let first = futures::stream::once(async move {
        Ok::<_, Infallible>(
            Event::default()
                .event("snapshot")
                .data(BASE64.encode(&snapshot)),
        )
    });

    let tail = BroadcastStream::new(rx).map(move |item| {
        let event = match item {
            Ok(encoded) => Event::default().event("update").data(encoded.as_ref()),
            Err(BroadcastStreamRecvError::Lagged(_)) => {
                let snapshot = hub.state.lock().snapshot().unwrap_or_default();
                Event::default()
                    .event("snapshot")
                    .data(BASE64.encode(&snapshot))
            }
        };
        Ok::<_, Infallible>(event)
    });

    let stream: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> =
        Box::pin(first.chain(tail));
    Sse::new(stream).keep_alive(KeepAlive::default())
}
