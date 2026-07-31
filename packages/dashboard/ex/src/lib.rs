//! Elixir binding for the dashboard [`Hub`] via unibind: an agent on the
//! BEAM opens a served CRDT document, publishes panes into it, and is woken
//! when the document changes underneath it.
//!
//! Documents cross the boundary as string ids rather than BEAM resources,
//! for the reason packages/tui/ex gives: `open` legitimately blocks (it
//! binds a listener and starts an HTTP server), unibind constructors cannot
//! be `blocking`, and an id survives a workspace checkpoint where an opaque
//! reference would not. A resource would be worse here than there -- BEAM
//! garbage collection of the handle would silently take the served
//! dashboard down with it. The process-global registry keeps every open
//! document alive until `close`.
//!
//! One constraint of the Elixir backend shapes the surface: Rust->BEAM push
//! exists only as a non-async free fn returning `UniStream<T>`, whose items
//! go to the *calling* pid, fixed at call time, under granted demand.
//! `watch` is that function; the Elixir side must call it from the process
//! that wants the messages.
//!
//! CRDT payloads cross as `Vec<u8>` and arrive as Elixir binaries. That is
//! new: the ex backend used to reject binary payloads outright, and the
//! first cut of this crate base64'd every snapshot and update through a
//! `String`, paying a copy and 33% of the bytes on the hot path for nothing.
//!
//! ## The mirror document, and how it stays current
//!
//! A human's edit reaches the hub through the browser's `POST /apply`, i.e.
//! `Hub::import`. This crate keeps a mirror `LoroDoc` per document because
//! Loro reports a change only to a document that just imported it, and the
//! watch stream is built from those change events.
//!
//! The mirror is driven by `Hub::subscribe`: one snapshot to seed it, taken
//! under the hub's lock together with the update stream that continues it,
//! and then one incremental import per delta. It used to be a 200ms timer
//! re-exporting and re-importing the whole document, because the hub's
//! update broadcast was `pub(crate)` and nothing public reported that a
//! merge had happened (ENG-10199). An idle document now costs nothing and a
//! human's edit wakes the agent as soon as it lands rather than up to a
//! poll interval later.
//!
//! ## What the events say about authorship
//!
//! A Loro change event reports *what* changed (root container, path, shape
//! of the diff) and never who. `Update::authors` carries that separately:
//! the peers whose ops are in the delta, taken from the version-vector diff
//! at commit time. Compared against `Hub::peer_id` it separates this side's
//! own `apply_scope` echo from a browser's merge, which is what stops an
//! agent being woken by its own writes.

/// The exported boundary. The module name names the generated Elixir
/// namespace (`DashboardEx`) and the OTP app (`:dashboard_ex`).
#[unibind::export(backends(ex))]
mod _dashboard_ex {
    use std::collections::{BTreeSet, HashMap};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::pin::Pin;
    use std::sync::{Arc, OnceLock, Weak};
    use std::task::{Context, Poll};

    use dashboard_core::{Dashboard, Hub, Merge, Pane, ServedDashboard, serve_hub};
    use futures::Stream;
    use loro::event::{Diff, DiffEvent};
    use loro::{ContainerID, Index, LoroDoc, ToJson as _};
    use parking_lot::Mutex;
    use tokio::sync::broadcast;
    use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
    use tokio_stream::wrappers::UnboundedReceiverStream;
    use unibind_runtime::UniStream;



    /// Boundary failures. A document simply having no panes, or a merge
    /// landing as `pending`, is data rather than an error.
    #[unibind::error]
    #[derive(Debug)]
    pub enum DashboardError {
        /// The dashboard server could not be started (port in use, no
        /// permission to bind).
        Serve {
            /// What the bind or serve attempt reported.
            message: String,
        },
        /// No open document has this id (never opened, or `close`d).
        NotFound {
            /// The unknown id.
            message: String,
        },
        /// `open` was called with an id that is already open.
        AlreadyOpen {
            /// The id in use.
            message: String,
        },
        /// The CRDT layer refused the operation.
        Crdt {
            /// What Loro or the hub reported.
            message: String,
        },
        /// An argument could not be decoded: a data pane's body was not
        /// JSON.
        BadInput {
            /// What was wrong with the input.
            message: String,
        },
    }

    unibind_ex_runtime::message_error!(DashboardError {
        Serve,
        NotFound,
        AlreadyOpen,
        Crdt,
        BadInput,
    });

    /// One container's worth of change in a document, as `watch` pushes it.
    ///
    /// Deliberately a summary rather than the diff itself: the consumer is a
    /// BEAM process being woken, and the authoritative content is one
    /// `value/1` or `snapshot/1` away. Carrying the whole delta would put
    /// the document's text through the NIF boundary on every keystroke.
    #[unibind::record]
    #[derive(Clone)]
    pub struct DocEvent {
        /// The document id the change belongs to.
        pub doc: String,
        /// Root container the change happened under: `panes` for anything a
        /// producer publishes, any other name for a container a client
        /// created.
        pub root: String,
        /// Slash-joined path from the root to the changed container, empty
        /// when the root itself changed.
        pub path: String,
        /// Diff shape: `map`, `text`, `list`, `tree`, or `unknown`.
        pub kind: String,
        /// For a map diff, the keys whose values changed, sorted. Empty for
        /// every other shape.
        pub keys: Vec<String>,
        /// Characters (text) or elements (list) inserted.
        pub inserted: usize,
        /// Characters (text) or elements (list) deleted.
        pub deleted: usize,
    }

    /// One open document: the hub, its HTTP server, the panes this side has
    /// published, and the mirror the watch stream reads.
    struct Document {
        hub: Arc<Hub>,
        url: String,
        /// Dropping a `Dashboard` shuts its HTTP server down, so the server's
        /// lifetime is exactly this struct's: `close` removes the registry
        /// entry and the server stops with it.
        _dashboard: Dashboard,
        /// Ids this side has published, so `panes/1` can answer "what did *I*
        /// put here" in a document several agents write to. Only the ids: the
        /// pane bodies live in the document, and `Hub::set_pane` touches one
        /// pane at a time, so there is nothing to re-send.
        panes: Mutex<BTreeSet<String>>,
        /// A peer of the hub's document, kept current by importing the hub's
        /// snapshot. Every read (`value`) and every watch event comes from
        /// here rather than from the hub, because Loro reports a change only
        /// to a document that just imported it.
        mirror: Mutex<LoroDoc>,
    }

    /// The event stream plus the Loro subscription feeding it.
    ///
    /// Loro unsubscribes on `Drop`, so the guard has to live exactly as long
    /// as the stream its callback writes into. Owning both here ties the two
    /// lifetimes together; leaking the guard with `mem::forget` would keep
    /// the callback (and its send half) alive for the process lifetime.
    struct Watch {
        events: UnboundedReceiverStream<DocEvent>,
        _subscription: loro::Subscription,
    }

    impl Stream for Watch {
        type Item = DocEvent;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Pin::new(&mut self.get_mut().events).poll_next(cx)
        }
    }

    /// What one container diff amounted to. A named struct rather than a
    /// tuple: the workspace denies `anonymous_tuple_return_type`.
    struct Summary {
        kind: &'static str,
        keys: Vec<String>,
        inserted: usize,
        deleted: usize,
    }

    /// Every open document, alive for the process lifetime unless `close`
    /// removes it -- the same registry shape packages/tui/ex uses for
    /// terminals.
    fn documents() -> &'static Mutex<HashMap<String, Arc<Document>>> {
        static DOCUMENTS: OnceLock<Mutex<HashMap<String, Arc<Document>>>> = OnceLock::new();
        DOCUMENTS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn document(id: &str) -> Result<Arc<Document>, DashboardError> {
        documents()
            .lock()
            .get(id)
            .map(Arc::clone)
            .ok_or_else(|| DashboardError::NotFound {
                message: format!("no open dashboard document with id {id:?}"),
            })
    }

    fn crdt_error(source: &impl std::fmt::Display) -> DashboardError {
        DashboardError::Crdt {
            message: source.to_string(),
        }
    }

    /// Pull the hub's current state into the mirror, which is what fires the
    /// watch subscription. Idempotent: importing ops the mirror already has
    /// changes nothing and emits nothing.
    fn refresh(document: &Document) -> Result<(), DashboardError> {
        let snapshot = document.hub.export_snapshot();
        if snapshot.is_empty() {
            // `Hub::export_snapshot` swallows an export failure as empty
            // bytes (`unwrap_or_default`), and Loro rejects an empty import
            // as a parse error -- so an export that failed would surface here
            // as a bogus decode error rather than as the no-op it is.
            return Ok(());
        }
        // The subscription callback runs synchronously inside `import`, still
        // under this lock. It only pushes into an unbounded channel, so it
        // neither blocks nor re-enters the mirror.
        let mirror = document.mirror.lock();
        mirror.import(&snapshot).map_err(|error| crdt_error(&error))?;
        Ok(())
    }

    /// Publish one pane and record that we own its id.
    ///
    /// No refresh: `Hub::set_pane` touches only this pane, the pump imports the
    /// resulting delta, and every read that needs the mirror current
    /// (`value/1`, `sync/1`) refreshes for itself. Refreshing here would put a
    /// whole-document export and import back on the write path, which is what
    /// this crate just stopped doing.
    fn publish(document: &Document, pane: Pane) {
        document.panes.lock().insert(pane.id.clone());
        document.hub.set_pane(&pane);
    }

    /// The root container a diff happened under. A diff's `path` starts at
    /// the root, so the root is the first hop's container, or the target
    /// itself when the root container is what changed.
    fn root_name(target: &ContainerID, path: &[(ContainerID, Index)]) -> String {
        let root = path.first().map_or(target, |(container, _)| container);
        match root {
            ContainerID::Root { name, .. } => name.to_string(),
            // A nested container can only be reached through a root, so this
            // is unreachable in practice; an empty root reads as "unknown"
            // rather than inventing a name.
            ContainerID::Normal { .. } => String::new(),
        }
    }

    fn path_string(path: &[(ContainerID, Index)]) -> String {
        path.iter()
            .map(|(_, index)| match index {
                Index::Key(key) => key.to_string(),
                Index::Seq(at) => at.to_string(),
                Index::Node(node) => node.to_string(),
            })
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Summarize one container diff.
    ///
    /// Read through `EnumAsInner`'s accessors rather than a `match`: loro's
    /// `Diff` gains and loses variants with cargo features (`Counter` rides
    /// the default `counter` feature), and a match over it would break on a
    /// feature change somewhere else in the workspace.
    fn summarize(diff: &Diff<'_>) -> Summary {
        let empty = |kind| Summary {
            kind,
            keys: Vec::new(),
            inserted: 0,
            deleted: 0,
        };
        if let Some(deltas) = diff.as_text() {
            let mut summary = empty("text");
            for delta in deltas {
                match delta {
                    loro::TextDelta::Insert { insert, .. } => {
                        summary.inserted += insert.chars().count();
                    }
                    loro::TextDelta::Delete { delete } => summary.deleted += delete,
                    loro::TextDelta::Retain { .. } => {}
                }
            }
            return summary;
        }
        if let Some(map) = diff.as_map() {
            let mut keys: Vec<String> = map.updated.keys().map(ToString::to_string).collect();
            // Sorted so an event is reproducible: the delta is a hash map.
            keys.sort_unstable();
            return Summary {
                kind: "map",
                keys,
                inserted: 0,
                deleted: 0,
            };
        }
        if let Some(items) = diff.as_list() {
            let mut summary = empty("list");
            for item in items {
                match item {
                    loro::event::ListDiffItem::Insert { insert, .. } => {
                        summary.inserted += insert.len();
                    }
                    loro::event::ListDiffItem::Delete { delete } => summary.deleted += delete,
                    loro::event::ListDiffItem::Retain { .. } => {}
                }
            }
            return summary;
        }
        if diff.as_tree().is_some() {
            return empty("tree");
        }
        empty("unknown")
    }

    fn doc_events(id: &str, event: &DiffEvent<'_>) -> Vec<DocEvent> {
        event
            .events
            .iter()
            .map(|diff| {
                let summary = summarize(&diff.diff);
                DocEvent {
                    doc: id.to_owned(),
                    root: root_name(diff.target, diff.path),
                    path: path_string(diff.path),
                    kind: summary.kind.to_owned(),
                    keys: summary.keys,
                    inserted: summary.inserted,
                    deleted: summary.deleted,
                }
            })
            .collect()
    }

    /// Keep `document`'s mirror current until the document is closed.
    ///
    /// Driven by the hub's update stream, not a timer: `Hub::subscribe` hands
    /// back the seed snapshot and the receiver that continues it, both taken
    /// under one hub lock, so there is no window where a delta lands between
    /// the two and is lost.
    ///
    /// A dedicated OS thread rather than a tokio task because a Loro import is
    /// CPU work that would stall a runtime worker shared with the dashboard's
    /// HTTP server, and because `blocking_recv` parks the thread while the
    /// document is idle. Shutdown needs no flag: `close` drops the last strong
    /// `Arc<Document>`, which drops the hub and its broadcast sender, and the
    /// receiver then reports `Closed`.
    fn pump(document: &Arc<Document>) {
        let weak = Arc::downgrade(document);
        let (snapshot, mut updates) = document.hub.subscribe().into_parts();
        // Seed before the loop so the first delta applies to a document that
        // already holds everything before it.
        if !snapshot.is_empty() {
            drop(document.mirror.lock().import(&snapshot));
        }
        std::thread::spawn(move || {
            loop {
                let received = updates.blocking_recv();
                // Upgrade after the wait, not before: holding a strong
                // reference across it would keep the document alive forever.
                let Some(document) = Weak::upgrade(&weak) else {
                    return;
                };
                match received {
                    // The callback that feeds the watch stream runs
                    // synchronously inside `import`, under this lock. It only
                    // pushes into an unbounded channel, so it neither blocks
                    // nor re-enters the mirror.
                    Ok(update) => {
                        drop(document.mirror.lock().import(update.bytes()));
                    }
                    // The mirror fell further behind than the broadcast ring is
                    // deep, so the deltas it missed are gone. Re-seed from a
                    // whole snapshot rather than carrying on and silently
                    // diverging from the hub.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        drop(refresh(&document));
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    /// Open document `doc` and serve it over HTTP on `127.0.0.1:port`;
    /// returns the URL to open in a browser. `port` 0 asks the OS for a free
    /// one, which the returned URL then names. Blocking (DirtyIo): binds a
    /// listener and starts the server.
    #[unibind(blocking)]
    pub fn open(doc: String, #[unibind(default = 0)] port: u16) -> Result<String, DashboardError> {
        let hub = Hub::new();
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let runtime = unibind_ex_runtime::runtime();
        // The NIF runs on a BEAM dirty scheduler thread, never on a tokio
        // worker, so blocking it on the bind is safe.
        let ServedDashboard {
            dashboard,
            shutdown: _,
        } = runtime
            .block_on(serve_hub(Arc::clone(&hub), addr, None, runtime.handle()))
            .map_err(|error| DashboardError::Serve {
                message: error.to_string(),
            })?;
        let url = dashboard.url();
        let document = Arc::new(Document {
            hub,
            url: url.clone(),
            _dashboard: dashboard,
            panes: Mutex::new(BTreeSet::new()),
            mirror: Mutex::new(LoroDoc::new()),
        });

        // Claim the id only now, so a lost race drops the freshly served
        // dashboard (stopping its server) instead of orphaning it.
        {
            let mut open = documents().lock();
            if open.contains_key(&doc) {
                return Err(DashboardError::AlreadyOpen {
                    message: format!("document {doc:?} is already open"),
                });
            }
            open.insert(doc, Arc::clone(&document));
        }
        pump(&document);
        Ok(url)
    }

    /// Close the document: stop its server, drop its panes, and stop
    /// resolving its id. Watch streams over it end once their queue drains.
    #[unibind(blocking)]
    pub fn close(doc: String) -> Result<(), DashboardError> {
        documents()
            .lock()
            .remove(&doc)
            .map(drop)
            .ok_or_else(|| DashboardError::NotFound {
                message: format!("no open dashboard document with id {doc:?}"),
            })
    }

    /// Ids of every open document in this process.
    pub fn list() -> Vec<String> {
        documents().lock().keys().cloned().collect()
    }

    /// The browser URL of an open document.
    pub fn url(doc: String) -> Result<String, DashboardError> {
        Ok(document(&doc)?.url.clone())
    }

    /// Publish (or replace) an HTML pane. The producer ships its own markup;
    /// the browser mounts it sandboxed.
    #[unibind(blocking)]
    pub fn set_html(
        doc: String,
        pane: String,
        title: String,
        html: String,
    ) -> Result<(), DashboardError> {
        let document = document(&doc)?;
        publish(&document, Pane::html(pane, title, html));
        Ok(())
    }

    /// Publish (or replace) a data pane: `json` is parsed here so a
    /// malformed body is a typed error instead of a pane rendering the word
    /// `null`. `renderer` names the frontend renderer; an unknown name falls
    /// back to a generic tree.
    #[unibind(blocking)]
    pub fn set_data(
        doc: String,
        pane: String,
        title: String,
        renderer: String,
        json: String,
    ) -> Result<(), DashboardError> {
        let document = document(&doc)?;
        let data: serde_json::Value =
            serde_json::from_str(&json).map_err(|error| DashboardError::BadInput {
                message: format!("data pane body is not JSON: {error}"),
            })?;
        publish(&document, Pane::data(pane, title, renderer, data));
        Ok(())
    }

    /// Remove one pane this side published. Unknown ids are a no-op: the
    /// caller's intent (the pane is gone) already holds.
    #[unibind(blocking)]
    pub fn drop_pane(doc: String, pane: String) -> Result<(), DashboardError> {
        let document = document(&doc)?;
        document.panes.lock().remove(&pane);
        document.hub.drop_pane(&pane);
        Ok(())
    }

    /// Ids of the panes this side has published into `doc`.
    pub fn panes(doc: String) -> Result<Vec<String>, DashboardError> {
        Ok(document(&doc)?.panes.lock().iter().cloned().collect())
    }

    /// A full Loro snapshot of the document, oplog included, so the receiver
    /// can replay any past version. Arrives on the BEAM as a binary.
    #[unibind(blocking)]
    pub fn snapshot(doc: String) -> Result<Vec<u8>, DashboardError> {
        Ok(document(&doc)?.hub.export_snapshot())
    }

    /// Merge a Loro update or snapshot into the document and fan it out to
    /// every connected browser.
    ///
    /// Returns `"applied"` when every op landed, or `"pending"` when the
    /// update depends on ops this document does not have -- Loro recorded
    /// them but the edit is invisible until the missing range arrives, so
    /// `"pending"` means "send me the rest", not "done".
    #[unibind(blocking)]
    pub fn merge(doc: String, update: &[u8]) -> Result<String, DashboardError> {
        let document = document(&doc)?;
        let merged = document
            .hub
            .import(update)
            .map_err(|error| crdt_error(&error))?;
        refresh(&document)?;
        Ok(match merged {
            Merge::Applied => "applied",
            Merge::Pending => "pending",
        }
        .to_owned())
    }

    /// The whole document as JSON: every root container resolved to plain
    /// values. This is how a woken agent reads what a human typed without
    /// decoding CRDT bytes on the BEAM.
    #[unibind(blocking)]
    pub fn value(doc: String) -> Result<String, DashboardError> {
        let document = document(&doc)?;
        refresh(&document)?;
        let json = document.mirror.lock().get_deep_value().to_json();
        Ok(json)
    }

    /// Pull the hub's current state into the mirror now instead of waiting
    /// for the next poll tick. Every read already does this; it is exported
    /// so a test can assert on a watch event without sleeping.
    #[unibind(blocking)]
    pub fn sync(doc: String) -> Result<(), DashboardError> {
        let document = document(&doc)?;
        refresh(&document)
    }

    /// Stream every change to `doc` to the calling process.
    ///
    /// Items go to the pid that called this function, fixed at call time, and
    /// only under demand granted through the generated `unibind_demand/2`.
    /// The generated Elixir wrapper blocks on a bare `receive`, so a
    /// GenServer must call `DashboardEx.Native.watch/2` itself and handle
    /// `{:unibind_stream, ref, {:item, event}}` in `handle_info/2` -- and it
    /// must keep the returned handle, because collecting it aborts the
    /// producer.
    ///
    /// Every change is reported, including this side's own pane publishes:
    /// see the module docs on what the events do not say.
    pub fn watch(doc: String) -> Result<UniStream<DocEvent>, DashboardError> {
        let document = document(&doc)?;
        let (sender, receiver) = unbounded_channel();
        let id = doc;
        // Unbounded on purpose: the callback runs inside Loro's own commit
        // path, where blocking would stall the import (and, through it, the
        // dashboard's HTTP handler). Demand backpressure lives one layer up,
        // in the generated stream handle.
        let sink: UnboundedSender<DocEvent> = sender;
        let subscription = document.mirror.lock().subscribe_root(Arc::new(move |event| {
            for item in doc_events(&id, &event) {
                // A closed receiver means the BEAM side dropped the stream;
                // the subscription is about to be dropped with it.
                drop(sink.send(item));
            }
        }));
        Ok(UniStream::new(Watch {
            events: UnboundedReceiverStream::new(receiver),
            _subscription: subscription,
        }))
    }
}
