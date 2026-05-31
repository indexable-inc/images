//! The shared Loro document and its SSE fan-out.
//!
//! [`Hub`] owns one [`LoroDoc`] whose root `panes` map holds one entry per pane,
//! keyed by `"<scope>\x1f<id>"`. A scope is one frame source: the in-process
//! dashboard uses a single scope, the aggregator uses one per producer.
//! [`Hub::apply_scope`] reconciles exactly the entries under a scope and leaves
//! every other scope untouched, so independent producers never delete each
//! other's panes.
//!
//! Each pane is a `meta` [`LoroMap`] of scalars (`kind`, `title`, plus the
//! kind's own fields) and a `body` [`LoroText`] holding the one large mutable
//! field: a terminal's screen, an HTML document, or a data view's JSON. Storing
//! the body as text means updates diff incrementally and, because a Loro oplog
//! *is* a recording, the whole pane history replays for free regardless of kind.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use loro::{ExportMode, LoroDoc, LoroMap, LoroText, VersionVector};
use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::pane::{Pane, TerminalView, View};
use crate::{Error, Result};

/// How many CRDT updates a slow SSE client may fall behind before it is fed a
/// fresh snapshot instead. Bounds memory per connection.
const BROADCAST_CAPACITY: usize = 256;

/// The unit separator joining a scope to a pane id in the root map. Neither a
/// scope (`"<pid>-<uuid>"` or the in-process scope) nor a pane id (a UUID)
/// contains it, so the split back into `(scope, id)` is unambiguous.
const SCOPE_SEP: char = '\u{1f}';

fn doc_key(scope: &str, id: &str) -> String {
    format!("{scope}{SCOPE_SEP}{id}")
}

/// The one large mutable field of a view, stored in the pane's `body` text. The
/// browser interprets it by the pane's `kind`: ANSI screen, HTML, or JSON.
fn body_of(view: &View) -> String {
    match view {
        View::Terminal(t) => t.screen.clone(),
        View::Html(h) => h.html.clone(),
        // A data view's JSON is canonicalized to text so it diffs and replays
        // like any other body; the frontend parses it back.
        View::Data(d) => serde_json::to_string(&d.data).unwrap_or_default(),
    }
}

/// The Loro handles backing one pane card, plus the scalar values already
/// written, cached across applies so a tick only re-inserts a value that
/// changed (an unchanged insert is still a CRDT op, so caching is what keeps an
/// idle pane from producing a delta).
struct Slot {
    meta: LoroMap,
    body: LoroText,
    /// The view tag this slot was created for. A producer that reuses an id with
    /// a different kind triggers a recreate rather than a confused in-place edit.
    kind: &'static str,
    title: String,
    subtitle: String,
    // Terminal-only scalars; untouched (and unwritten) for other kinds. The
    // command and args are not stored separately: `Pane::terminal` carries them
    // as the title and subtitle, which every pane already has.
    rows: i64,
    cols: i64,
    alive: bool,
    cursor_row: i64,
    cursor_col: i64,
    cursor_visible: bool,
    cursor_shape: String,
    exit_code: Option<i32>,
    // Data-only scalar.
    renderer: String,
    body_str: String,
}

/// The shared document plus the per-pane handles and the version already
/// streamed to live clients.
struct DocState {
    doc: LoroDoc,
    root: LoroMap,
    panes: HashMap<String, Slot>,
    streamed: VersionVector,
}

impl DocState {
    fn new() -> Self {
        let doc = LoroDoc::new();
        let root = doc.get_map("panes");
        let streamed = doc.oplog_vv();
        Self {
            doc,
            root,
            panes: HashMap::new(),
            streamed,
        }
    }

    /// Reconcile the panes under `scope` to exactly `panes`. Entries under other
    /// scopes are left alone. Returns the CRDT delta since the last broadcast
    /// when anything changed.
    fn apply_scope(&mut self, scope: &str, panes: &[Pane]) -> Result<Option<Vec<u8>>> {
        for pane in panes {
            let key = doc_key(scope, &pane.id);
            let kind = pane.view.kind();

            // A reused id whose kind changed cannot be edited in place: the
            // stored scalars and body mean something different now. Drop the old
            // entry so the create path below rebuilds it cleanly.
            if self
                .panes
                .get(&key)
                .is_some_and(|slot| slot.kind != kind)
            {
                self.drop_keys(std::slice::from_ref(&key))?;
            }

            if !self.panes.contains_key(&key) {
                self.create_slot(&key, pane)?;
            }
            self.update_slot(&key, pane)?;
        }

        let prefix = format!("{scope}{SCOPE_SEP}");
        let live: HashSet<String> = panes.iter().map(|p| doc_key(scope, &p.id)).collect();
        let dead: Vec<String> = self
            .panes
            .keys()
            .filter(|key| key.starts_with(&prefix) && !live.contains(*key))
            .cloned()
            .collect();
        self.drop_keys(&dead)?;

        self.commit_delta()
    }

    /// Create the Loro containers for a new pane and cache them. The scalars and
    /// body are written by the [`update_slot`](Self::update_slot) call that
    /// always follows; this only establishes the map, the body text, and the
    /// immutable `kind`.
    fn create_slot(&mut self, key: &str, pane: &Pane) -> Result<()> {
        let meta = self
            .root
            .insert_container(key, LoroMap::new())
            .map_err(loro_err)?;
        meta.insert("kind", pane.view.kind()).map_err(loro_err)?;
        let body = meta
            .insert_container("body", LoroText::new())
            .map_err(loro_err)?;
        self.panes.insert(
            key.to_owned(),
            Slot {
                meta,
                body,
                kind: pane.view.kind(),
                // Sentinels that force the first update_slot to write every
                // field. `rows`/`cols` use -1 (a real size is >= 0); the strings
                // start empty so any real value differs; `alive` starts false so
                // a live pane writes `true` and a dead one writes `false` (the
                // cache and the doc both being "unset" would otherwise leave a
                // dead pane with no `alive` key, which the frontend reads as
                // alive).
                title: sentinel(),
                subtitle: sentinel(),
                rows: -1,
                cols: -1,
                alive: false,
                cursor_row: -1,
                cursor_col: -1,
                cursor_visible: false,
                cursor_shape: sentinel(),
                exit_code: None,
                renderer: sentinel(),
                body_str: sentinel(),
            },
        );
        // `alive` defaults to false in the sentinel; a pane that is in fact dead
        // would then be skipped by the diff and never written. Pre-write it so
        // the first update reconciles from a known doc state.
        if let View::Terminal(_) = &pane.view {
            let slot = self.panes.get(key).expect("slot inserted above");
            slot.meta.insert("alive", false).map_err(loro_err)?;
        }
        Ok(())
    }

    /// Reconcile one existing pane's scalars and body to `pane`, writing only the
    /// values that changed so an idle pane produces no delta.
    fn update_slot(&mut self, key: &str, pane: &Pane) -> Result<()> {
        let slot = self.panes.get_mut(key).expect("slot exists");
        if slot.title != pane.title {
            slot.meta.insert("title", pane.title.as_str()).map_err(loro_err)?;
            slot.title.clone_from(&pane.title);
        }
        if slot.subtitle != pane.subtitle {
            slot.meta
                .insert("subtitle", pane.subtitle.as_str())
                .map_err(loro_err)?;
            slot.subtitle.clone_from(&pane.subtitle);
        }
        match &pane.view {
            View::Terminal(t) => update_terminal(slot, t)?,
            View::Html(_) => {}
            View::Data(d) => {
                if slot.renderer != d.renderer {
                    slot.meta
                        .insert("renderer", d.renderer.as_str())
                        .map_err(loro_err)?;
                    slot.renderer.clone_from(&d.renderer);
                }
            }
        }
        let body = body_of(&pane.view);
        if slot.body_str != body {
            slot.body
                .update(&body, loro::UpdateOptions::default())
                .map_err(|source| Error::Dashboard {
                    message: format!("body text update: {source}"),
                })?;
            slot.body_str = body;
        }
        Ok(())
    }

    /// Drop every pane under `scope` (its producer disconnected). Returns the
    /// delta when the scope held anything.
    fn remove_scope(&mut self, scope: &str) -> Result<Option<Vec<u8>>> {
        let prefix = format!("{scope}{SCOPE_SEP}");
        let dead: Vec<String> = self
            .panes
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect();
        if dead.is_empty() {
            return Ok(None);
        }
        self.drop_keys(&dead)?;
        self.commit_delta()
    }

    fn drop_keys(&mut self, keys: &[String]) -> Result<()> {
        for key in keys {
            self.root.delete(key).map_err(loro_err)?;
            self.panes.remove(key);
        }
        Ok(())
    }

    /// Commit the pending edits and export the delta since the last broadcast,
    /// or `None` when nothing changed.
    fn commit_delta(&mut self) -> Result<Option<Vec<u8>>> {
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

/// A cache sentinel that no real scalar value equals, forcing the first write.
fn sentinel() -> String {
    "\u{0}<unset>".to_owned()
}

/// Reconcile a terminal view's scalars into its slot, writing only what changed.
/// The command and args are not written here: they ride in the pane's title and
/// subtitle, reconciled by [`DocState::update_slot`].
fn update_terminal(slot: &mut Slot, t: &TerminalView) -> Result<()> {
    let rows = i64::from(t.rows);
    let cols = i64::from(t.cols);
    if slot.rows != rows || slot.cols != cols {
        write_size(&slot.meta, t)?;
        slot.rows = rows;
        slot.cols = cols;
    }
    if slot.alive != t.alive {
        slot.meta.insert("alive", t.alive).map_err(loro_err)?;
        slot.alive = t.alive;
    }
    let cursor_row = i64::from(t.cursor_row);
    let cursor_col = i64::from(t.cursor_col);
    if slot.cursor_row != cursor_row
        || slot.cursor_col != cursor_col
        || slot.cursor_visible != t.cursor_visible
        || slot.cursor_shape != t.cursor_shape
    {
        write_cursor(&slot.meta, t)?;
        slot.cursor_row = cursor_row;
        slot.cursor_col = cursor_col;
        slot.cursor_visible = t.cursor_visible;
        slot.cursor_shape.clone_from(&t.cursor_shape);
    }
    if slot.exit_code != t.exit_code {
        write_exit(&slot.meta, t)?;
        slot.exit_code = t.exit_code;
    }
    Ok(())
}

fn loro_err(source: impl std::fmt::Display) -> Error {
    Error::Dashboard {
        message: source.to_string(),
    }
}

/// Write a terminal view's size into its meta map. One owner for the size keys
/// so the insert and per-tick update paths cannot disagree.
fn write_size(meta: &LoroMap, t: &TerminalView) -> Result<()> {
    meta.insert("rows", i64::from(t.rows)).map_err(loro_err)?;
    meta.insert("cols", i64::from(t.cols)).map_err(loro_err)
}

/// Write a terminal view's cursor fields into its meta map.
fn write_cursor(meta: &LoroMap, t: &TerminalView) -> Result<()> {
    meta.insert("cursor_row", i64::from(t.cursor_row)).map_err(loro_err)?;
    meta.insert("cursor_col", i64::from(t.cursor_col)).map_err(loro_err)?;
    meta.insert("cursor_visible", t.cursor_visible).map_err(loro_err)?;
    meta.insert("cursor_shape", t.cursor_shape.as_str()).map_err(loro_err)
}

/// Write a terminal view's exit code into its meta map: an `i64` when the process
/// exited with a code, otherwise absent (still running, or signalled).
fn write_exit(meta: &LoroMap, t: &TerminalView) -> Result<()> {
    t.exit_code.map_or_else(
        // A signalled or still-running process has no code; clear any prior value
        // so a re-spawned id under the same key never shows a stale exit code.
        // delete on an absent key is a harmless no-op.
        || meta.delete("exit_code").map_err(loro_err),
        |code| meta.insert("exit_code", i64::from(code)).map_err(loro_err),
    )
}

/// Owns the shared document and fans CRDT updates out to SSE subscribers.
///
/// One hub backs any number of frame sources. The in-process dashboard drives it
/// from a poll loop over a `TuiManager`; the aggregator drives it from many
/// unix-socket readers. Both call [`apply_scope`](Self::apply_scope) and
/// [`remove_scope`](Self::remove_scope); the hub serializes them under one lock.
pub struct Hub {
    state: Mutex<DocState>,
    updates: broadcast::Sender<Arc<str>>,
}

impl Hub {
    /// A fresh hub with an empty document and no subscribers.
    #[must_use]
    pub fn new() -> Arc<Self> {
        let (updates, _) = broadcast::channel(BROADCAST_CAPACITY);
        Arc::new(Self {
            state: Mutex::new(DocState::new()),
            updates,
        })
    }

    /// Reconcile the panes under `scope` to exactly `panes` and broadcast the
    /// resulting delta. A failed apply is dropped: the next tick re-renders.
    pub fn apply_scope(&self, scope: &str, panes: &[Pane]) {
        let delta = self.state.lock().apply_scope(scope, panes);
        self.broadcast(delta);
    }

    /// Drop every pane under `scope` and broadcast the delta.
    pub fn remove_scope(&self, scope: &str) {
        let delta = self.state.lock().remove_scope(scope);
        self.broadcast(delta);
    }

    fn broadcast(&self, delta: Result<Option<Vec<u8>>>) {
        if let Ok(Some(bytes)) = delta {
            let encoded: Arc<str> = Arc::from(BASE64.encode(&bytes).as_str());
            let _ = self.updates.send(encoded);
        }
    }

    /// Subscribe to the base64 CRDT update stream and read the current full
    /// snapshot, both under one lock so the snapshot version lines up with the
    /// first update the subscriber will receive.
    pub(crate) fn subscribe(&self) -> (Vec<u8>, broadcast::Receiver<Arc<str>>) {
        let state = self.state.lock();
        let rx = self.updates.subscribe();
        (state.snapshot().unwrap_or_default(), rx)
    }

    /// A base64 full snapshot, for a client the broadcast stream outran.
    pub(crate) fn snapshot_b64(&self) -> String {
        BASE64.encode(self.state.lock().snapshot().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::TerminalView;

    fn terminal(id: &str, screen: &str) -> Pane {
        Pane::terminal(
            id,
            TerminalView {
                command: "cat".to_owned(),
                args: String::new(),
                rows: 24,
                cols: 80,
                alive: true,
                screen: screen.to_owned(),
                cursor_row: 0,
                cursor_col: 0,
                cursor_visible: true,
                cursor_shape: "block".to_owned(),
                exit_code: None,
            },
        )
    }

    /// The core multi-producer invariant: one producer's reconcile never touches
    /// another's panes, and dropping a producer removes only its own.
    #[test]
    fn scopes_do_not_clobber_each_other() {
        let mut state = DocState::new();
        state
            .apply_scope("a", &[terminal("1", "x"), terminal("2", "y")])
            .unwrap();
        state.apply_scope("b", &[terminal("3", "z")]).unwrap();
        assert_eq!(state.panes.len(), 3);

        // Reconciling scope a to a single pane drops a's other pane and leaves
        // scope b alone.
        state.apply_scope("a", &[terminal("1", "x")]).unwrap();
        assert_eq!(state.panes.len(), 2);
        assert!(state.panes.keys().any(|key| key.starts_with("b\u{1f}")));

        // Disconnecting producer a removes only a's panes.
        state.remove_scope("a").unwrap();
        assert_eq!(state.panes.len(), 1);
        assert!(state.panes.keys().all(|key| key.starts_with("b\u{1f}")));
    }

    /// A tick that changes nothing produces no delta, so idle producers do not
    /// spam every connected browser.
    #[test]
    fn unchanged_apply_yields_no_delta() {
        let mut state = DocState::new();
        assert!(state.apply_scope("a", &[terminal("1", "x")]).unwrap().is_some());
        assert!(state.apply_scope("a", &[terminal("1", "x")]).unwrap().is_none());
        // A screen change does produce a delta.
        assert!(state.apply_scope("a", &[terminal("1", "y")]).unwrap().is_some());
    }

    /// A runtime resize must re-write `rows`/`cols` even when the screen text is
    /// byte-identical: clients read size from the CRDT every render, so a
    /// size-only change has to reach the doc, not just the cache.
    #[test]
    fn resize_updates_size() {
        let mut state = DocState::new();
        state.apply_scope("a", &[terminal("1", "x")]).unwrap();

        let mut resized = terminal("1", "x");
        if let View::Terminal(t) = &mut resized.view {
            t.rows = 40;
            t.cols = 120;
        }
        assert!(state.apply_scope("a", &[resized]).unwrap().is_some());

        let key = doc_key("a", "1");
        let meta = &state.panes[&key].meta;
        let rows = meta.get("rows").unwrap().get_deep_value().into_i64().unwrap();
        let cols = meta.get("cols").unwrap().get_deep_value().into_i64().unwrap();
        assert_eq!((rows, cols), (40, 120));
    }

    /// Removing a scope that holds nothing is a no-op, not a spurious broadcast.
    #[test]
    fn removing_empty_scope_yields_no_delta() {
        let mut state = DocState::new();
        assert!(state.remove_scope("ghost").unwrap().is_none());
    }

    /// Heterogeneous panes coexist under one scope: a terminal, an HTML pane, and
    /// a data pane all land with the right `kind` and body, and an unchanged
    /// re-apply of the mixed set yields no delta.
    #[test]
    fn heterogeneous_panes_apply_and_idle() {
        let mut state = DocState::new();
        let panes = vec![
            terminal("t", "screen"),
            Pane::html("h", "notes", "<b>hi</b>"),
            Pane::data("d", "metrics", "gauge", serde_json::json!({"cpu": 0.5})),
        ];
        assert!(state.apply_scope("p", &panes).unwrap().is_some());
        assert_eq!(state.panes.len(), 3);

        let html = &state.panes[&doc_key("p", "h")];
        assert_eq!(html.kind, "html");
        assert_eq!(html.body.to_string(), "<b>hi</b>");

        let data = &state.panes[&doc_key("p", "d")];
        assert_eq!(data.kind, "data");
        assert_eq!(data.body.to_string(), r#"{"cpu":0.5}"#);

        // A byte-identical re-apply of the whole mixed set is silent.
        assert!(state.apply_scope("p", &panes).unwrap().is_none());
    }

    /// Reusing an id with a different kind recreates the pane rather than editing
    /// the wrong fields in place.
    #[test]
    fn kind_change_recreates_pane() {
        let mut state = DocState::new();
        state.apply_scope("p", &[terminal("x", "screen")]).unwrap();
        assert_eq!(state.panes[&doc_key("p", "x")].kind, "terminal");

        state
            .apply_scope("p", &[Pane::html("x", "now html", "<i>swapped</i>")])
            .unwrap();
        let slot = &state.panes[&doc_key("p", "x")];
        assert_eq!(slot.kind, "html");
        assert_eq!(slot.body.to_string(), "<i>swapped</i>");
    }
}
