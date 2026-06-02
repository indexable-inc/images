//! The shared Loro document and its SSE fan-out.
//!
//! [`Hub`] owns one [`LoroDoc`] whose root `panes` map holds one entry per pane,
//! keyed by `"<scope>\x1f<id>"`. A scope is one frame source: the in-process
//! dashboard uses a single scope, the aggregator uses one per producer.
//! [`Hub::apply_scope`] reconciles exactly the entries under a scope and leaves
//! every other scope untouched, so independent producers never delete each
//! other's panes.
//!
//! Each pane is a `meta` [`LoroMap`] of scalars (`kind`, `created_at`, `title`,
//! `subtitle`, plus the view's own scalar fields) and one [`LoroText`] per
//! large mutable field the view declares: a terminal's `body` screen, an HTML
//! `body`, or an execution's `source`/`stdout`/`stderr`/`result`. A view tells
//! the hub its shape through two projections, [`view_scalars`] and
//! [`view_texts`], so adding a resource kind never touches the reconcile loop.
//! Storing each field as text means updates diff incrementally and, because a
//! Loro oplog *is* a recording, the whole pane history replays for free.
//!
//! Every commit carries a millisecond wall-clock timestamp
//! ([`set_next_commit_timestamp`](LoroDoc::set_next_commit_timestamp)), and each
//! pane is stamped with a `created_at` the first time it appears. Together they
//! let a browser scrub the document to any past moment and show each resource's
//! age, with no producer opting in.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use loro::{ExportMode, LoroDoc, LoroMap, LoroText, VersionVector};
use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::pane::{Pane, View};
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

/// Milliseconds since the Unix epoch, saturating instead of panicking on a clock
/// before the epoch or past `i64::MAX`. Used for both per-pane `created_at` and
/// per-commit timestamps, so the timeline axis and a pane's age share one scale.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

/// A scalar meta value the hub reconciles into a pane's meta map. [`Absent`]
/// means "ensure the key is not present", which lets an optional field (a
/// terminal's exit code before it exits, an execution's `ok` while it runs) be
/// expressed uniformly with the present cases.
///
/// [`Absent`]: Scalar::Absent
#[derive(Clone, PartialEq)]
enum Scalar {
    Bool(bool),
    Int(i64),
    Str(String),
    Absent,
}

/// The scalar meta fields a view contributes, besides the common `kind`,
/// `created_at`, `title`, and `subtitle` that every pane carries.
///
/// The keys a view returns are fixed for its kind, so a slot created for one
/// kind always sees the same set on every later tick.
fn view_scalars(view: &View) -> Vec<(&'static str, Scalar)> {
    match view {
        View::Terminal(t) => vec![
            ("rows", Scalar::Int(i64::from(t.rows))),
            ("cols", Scalar::Int(i64::from(t.cols))),
            ("alive", Scalar::Bool(t.alive)),
            ("cursor_row", Scalar::Int(i64::from(t.cursor_row))),
            ("cursor_col", Scalar::Int(i64::from(t.cursor_col))),
            ("cursor_visible", Scalar::Bool(t.cursor_visible)),
            ("cursor_shape", Scalar::Str(t.cursor_shape.clone())),
            (
                "exit_code",
                t.exit_code
                    .map_or(Scalar::Absent, |code| Scalar::Int(i64::from(code))),
            ),
        ],
        View::Html(_) => Vec::new(),
        View::Exec(e) => vec![
            ("lang", Scalar::Str(e.lang.clone())),
            ("running", Scalar::Bool(e.running)),
            ("ok", e.ok.map_or(Scalar::Absent, Scalar::Bool)),
        ],
        View::Data(d) => vec![("renderer", Scalar::Str(d.renderer.clone()))],
    }
}

/// The large mutable text fields a view contributes, each stored in its own Loro
/// text container so it diffs and replays independently. The terminal's command
/// and args are not here: they ride in the pane's title and subtitle, which
/// every pane already has.
fn view_texts(view: &View) -> Vec<(&'static str, String)> {
    match view {
        View::Terminal(t) => vec![("body", t.screen.clone())],
        View::Html(h) => vec![("body", h.html.clone())],
        View::Exec(e) => vec![
            ("source", e.source.clone()),
            ("stdout", e.stdout.clone()),
            ("stderr", e.stderr.clone()),
            ("result", e.result.clone()),
        ],
        // A data view's JSON is canonicalized to text so it diffs and replays
        // like any other body; the frontend parses it back.
        View::Data(d) => vec![("body", serde_json::to_string(&d.data).unwrap_or_default())],
    }
}

/// Bodies up to this size are reconciled with Loro's refined text diff, which
/// produces small deltas for the common case (a terminal screen, a tweaked HTML
/// fragment, appended exec output). A larger body is replaced wholesale instead:
/// diffing two large, dissimilar strings is quadratic, and a body that changes
/// completely every tick (an HTML pane streaming a base64 image data URL) would
/// otherwise stall the single aggregator and bloat the oplog with edit ops.
const MAX_DIFF_BODY: usize = 32 * 1024;

/// Ceiling on the refined diff for a small body, so a pathological input can
/// never block the aggregator. On timeout the body falls back to a wholesale
/// replace, same as a large body.
const BODY_DIFF_TIMEOUT_MS: f64 = 50.0;

/// Reconcile a text field to `next`.
///
/// Small bodies use Loro's refined diff (cheap, small deltas) under a timeout;
/// large bodies, and any diff that times out, are replaced wholesale: delete the
/// current contents and insert the new ones, two bulk ops whose cost is
/// independent of how similar the two strings are.
fn set_text(text: &LoroText, next: &str) -> Result<()> {
    if next.len() <= MAX_DIFF_BODY {
        let options = loro::UpdateOptions {
            timeout_ms: Some(BODY_DIFF_TIMEOUT_MS),
            use_refined_diff: true,
        };
        if text.update(next, options).is_ok() {
            return Ok(());
        }
        // The diff timed out. It may have applied partial edits before bailing,
        // so the wholesale replace below reconciles against the container's own
        // current length rather than any cached previous body.
    }
    let current_len = text.len_unicode();
    if current_len > 0 {
        text.delete(0, current_len).map_err(loro_err)?;
    }
    text.insert(0, next).map_err(loro_err)?;
    Ok(())
}

/// One named text field of a pane: its container plus the last value written,
/// cached so an unchanged tick produces no op.
struct TextSlot {
    key: &'static str,
    text: LoroText,
    value: String,
}

/// The Loro handles backing one pane card, plus the scalar and text values
/// already written, cached across applies so a tick only re-inserts a value that
/// changed (an unchanged insert is still a CRDT op, so caching is what keeps an
/// idle pane from producing a delta).
struct Slot {
    meta: LoroMap,
    /// The view tag this slot was created for. A producer that reuses an id with
    /// a different kind triggers a recreate rather than a confused in-place edit.
    kind: &'static str,
    title: String,
    subtitle: String,
    /// Cached scalar meta, keyed by field name. Absent from the map until first
    /// written, so the first apply writes every field the view declares.
    scalars: HashMap<&'static str, Scalar>,
    /// One entry per text field the view declares, in creation order.
    texts: Vec<TextSlot>,
}

impl Slot {
    #[cfg(test)]
    fn text(&self, key: &str) -> String {
        self.texts
            .iter()
            .find(|slot| slot.key == key)
            .map_or_else(String::new, |slot| slot.text.to_string())
    }
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
        // Record a wall-clock timestamp on every change so the browser can map a
        // scrubber position to a document version. We also set an explicit
        // millisecond timestamp per commit (see `commit_delta`); enabling this
        // keeps any commit that bypasses that path timestamped too.
        doc.set_record_timestamp(true);
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
            // stored scalars and text fields mean something different now. Drop
            // the old entry so the create path below rebuilds it cleanly.
            if self.panes.get(&key).is_some_and(|slot| slot.kind != kind) {
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
    /// text fields are written by the [`update_slot`](Self::update_slot) call
    /// that always follows; this only establishes the map, the `kind`, the
    /// once-stamped `created_at`, and one empty text container per declared field.
    fn create_slot(&mut self, key: &str, pane: &Pane) -> Result<()> {
        let meta = self
            .root
            .insert_container(key, LoroMap::new())
            .map_err(loro_err)?;
        meta.insert("kind", pane.view.kind()).map_err(loro_err)?;
        // Stamp the creation time once, when the pane first appears in the
        // document, and never rewrite it. Every pane carries it, so the canvas
        // shows each resource's age with no producer opt-in.
        meta.insert("created_at", now_ms()).map_err(loro_err)?;
        let mut texts = Vec::new();
        for (text_key, _) in view_texts(&pane.view) {
            let text = meta
                .insert_container(text_key, LoroText::new())
                .map_err(loro_err)?;
            // A fresh container is empty, so a cached empty value matches it and
            // the first update writes only a non-empty initial body.
            texts.push(TextSlot {
                key: text_key,
                text,
                value: String::new(),
            });
        }
        self.panes.insert(
            key.to_owned(),
            Slot {
                meta,
                kind: pane.view.kind(),
                // Sentinels so the first update writes both common strings even
                // when the real value is empty.
                title: sentinel(),
                subtitle: sentinel(),
                scalars: HashMap::new(),
                texts,
            },
        );
        Ok(())
    }

    /// Reconcile one existing pane's scalars and text fields to `pane`, writing
    /// only the values that changed so an idle pane produces no delta.
    fn update_slot(&mut self, key: &str, pane: &Pane) -> Result<()> {
        let slot = self.panes.get_mut(key).expect("slot exists");
        if slot.title != pane.title {
            slot.meta
                .insert("title", pane.title.as_str())
                .map_err(loro_err)?;
            slot.title.clone_from(&pane.title);
        }
        if slot.subtitle != pane.subtitle {
            slot.meta
                .insert("subtitle", pane.subtitle.as_str())
                .map_err(loro_err)?;
            slot.subtitle.clone_from(&pane.subtitle);
        }
        for (field, scalar) in view_scalars(&pane.view) {
            if slot.scalars.get(field) != Some(&scalar) {
                write_scalar(&slot.meta, field, &scalar)?;
                slot.scalars.insert(field, scalar);
            }
        }
        for (field, next) in view_texts(&pane.view) {
            // Match by key rather than position: the key set is fixed per kind,
            // so the lookup always hits, and matching keeps the two projections
            // from silently drifting if one is reordered.
            if let Some(text_slot) = slot.texts.iter_mut().find(|slot| slot.key == field)
                && text_slot.value != next
            {
                set_text(&text_slot.text, &next)?;
                text_slot.value = next;
            }
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
    /// or `None` when nothing changed. Stamps the commit with a millisecond
    /// wall-clock timestamp so the browser timeline has a fine-grained axis.
    fn commit_delta(&mut self) -> Result<Option<Vec<u8>>> {
        self.doc.set_next_commit_timestamp(now_ms());
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

    /// A full snapshot of the current document, for a newly-connected client, a
    /// client that fell too far behind the update stream, or a persisted
    /// recording. Includes the complete oplog, so the receiver can replay any
    /// past version, not only the latest state.
    fn snapshot(&self) -> Result<Vec<u8>> {
        self.doc.export(ExportMode::Snapshot).map_err(loro_err)
    }
}

/// A cache sentinel that no real scalar value equals, forcing the first write.
fn sentinel() -> String {
    "\u{0}<unset>".to_owned()
}

/// Write one scalar meta field, or delete it when [`Scalar::Absent`]. Deleting an
/// absent key is a harmless no-op, so the first apply of an absent field is safe.
fn write_scalar(meta: &LoroMap, field: &str, scalar: &Scalar) -> Result<()> {
    match scalar {
        Scalar::Bool(value) => meta.insert(field, *value).map_err(loro_err),
        Scalar::Int(value) => meta.insert(field, *value).map_err(loro_err),
        Scalar::Str(value) => meta.insert(field, value.as_str()).map_err(loro_err),
        Scalar::Absent => meta.delete(field).map_err(loro_err),
    }
}

fn loro_err(source: impl std::fmt::Display) -> Error {
    Error::Dashboard {
        message: source.to_string(),
    }
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

    /// The full document snapshot bytes, including the complete oplog. Used by
    /// the recorder to persist a replayable recording to disk.
    #[must_use]
    pub fn export_snapshot(&self) -> Vec<u8> {
        self.state.lock().snapshot().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pane::{ExecView, TerminalView};

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

    fn meta_i64(state: &DocState, key: &str, field: &str) -> Option<i64> {
        state.panes[key]
            .meta
            .get(field)
            .and_then(|value| value.get_deep_value().into_i64().ok())
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
        assert_eq!(meta_i64(&state, &key, "rows"), Some(40));
        assert_eq!(meta_i64(&state, &key, "cols"), Some(120));
    }

    /// Every pane is stamped with a `created_at` once and only once: it appears
    /// after the first apply and does not change across later updates, so it
    /// reads as the dashboard-first-seen time.
    #[test]
    fn created_at_is_stamped_once() {
        let mut state = DocState::new();
        state.apply_scope("a", &[terminal("1", "x")]).unwrap();
        let key = doc_key("a", "1");
        let first = meta_i64(&state, &key, "created_at").expect("created_at present");

        // A later screen change must not move created_at.
        state.apply_scope("a", &[terminal("1", "y")]).unwrap();
        assert_eq!(meta_i64(&state, &key, "created_at"), Some(first));
    }

    /// Removing a scope that holds nothing is a no-op, not a spurious broadcast.
    #[test]
    fn removing_empty_scope_yields_no_delta() {
        let mut state = DocState::new();
        assert!(state.remove_scope("ghost").unwrap().is_none());
    }

    /// Heterogeneous panes coexist under one scope: a terminal, an HTML pane, an
    /// exec pane, and a data pane all land with the right `kind` and text fields,
    /// and an unchanged re-apply of the mixed set yields no delta.
    #[test]
    fn heterogeneous_panes_apply_and_idle() {
        let mut state = DocState::new();
        let panes = vec![
            terminal("t", "screen"),
            Pane::html("h", "notes", "<b>hi</b>"),
            Pane::exec(
                "e",
                ExecView {
                    source: "print('hi')".to_owned(),
                    lang: "python".to_owned(),
                    stdout: "hi\n".to_owned(),
                    stderr: String::new(),
                    result: String::new(),
                    running: false,
                    ok: Some(true),
                },
            ),
            Pane::data("d", "metrics", "gauge", serde_json::json!({"cpu": 0.5})),
        ];
        assert!(state.apply_scope("p", &panes).unwrap().is_some());
        assert_eq!(state.panes.len(), 4);

        let html = &state.panes[&doc_key("p", "h")];
        assert_eq!(html.kind, "html");
        assert_eq!(html.text("body"), "<b>hi</b>");

        let exec = &state.panes[&doc_key("p", "e")];
        assert_eq!(exec.kind, "exec");
        assert_eq!(exec.text("stdout"), "hi\n");
        assert_eq!(exec.text("source"), "print('hi')");

        let data = &state.panes[&doc_key("p", "d")];
        assert_eq!(data.kind, "data");
        assert_eq!(data.text("body"), r#"{"cpu":0.5}"#);

        // A byte-identical re-apply of the whole mixed set is silent.
        assert!(state.apply_scope("p", &panes).unwrap().is_none());
    }

    /// An execution streams from running to finished: the `running` flag flips,
    /// `ok` appears, and the captured output lands, each as a delta; re-applying
    /// the finished view is then silent.
    #[test]
    fn exec_running_then_finished() {
        let mut state = DocState::new();
        let running = Pane::exec(
            "e",
            ExecView {
                source: "subprocess.run(['echo', 'hi'])".to_owned(),
                lang: "python".to_owned(),
                stdout: String::new(),
                stderr: String::new(),
                result: String::new(),
                running: true,
                ok: None,
            },
        );
        assert!(state.apply_scope("p", &[running]).unwrap().is_some());
        let key = doc_key("p", "e");
        let ok_while_running = state.panes[&key].meta.get("ok");
        assert!(ok_while_running.is_none(), "ok is absent while running");

        let finished = Pane::exec(
            "e",
            ExecView {
                source: "subprocess.run(['echo', 'hi'])".to_owned(),
                lang: "python".to_owned(),
                stdout: "hi\n".to_owned(),
                stderr: String::new(),
                result: String::new(),
                running: false,
                ok: Some(true),
            },
        );
        let finished = std::slice::from_ref(&finished);
        assert!(state.apply_scope("p", finished).unwrap().is_some());
        assert_eq!(state.panes[&key].text("stdout"), "hi\n");
        assert!(state.panes[&key].meta.get("ok").is_some(), "ok present when done");

        // Re-applying the identical finished view produces nothing.
        assert!(state.apply_scope("p", finished).unwrap().is_none());
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
        assert_eq!(slot.text("body"), "<i>swapped</i>");
    }
}
