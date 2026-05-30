//! The shared Loro document and its SSE fan-out.
//!
//! [`Hub`] owns one [`LoroDoc`] whose root `terminals` map holds one entry per
//! terminal, keyed by `"<scope>\x1f<id>"`. A scope is one frame source: the
//! in-process dashboard uses a single scope, the aggregator uses one per
//! producer. [`Hub::apply_scope`] reconciles exactly the entries under a scope
//! and leaves every other scope untouched, so independent producers never
//! delete each other's terminals.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use loro::{ExportMode, LoroDoc, LoroMap, LoroText, VersionVector};
use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::frame::TerminalFrame;
use crate::{Error, Result};

/// How many CRDT updates a slow SSE client may fall behind before it is fed a
/// fresh snapshot instead. Bounds memory per connection.
const BROADCAST_CAPACITY: usize = 256;

/// The unit separator joining a scope to a terminal id in the root map. Neither
/// a scope (`"<pid>-<uuid>"` or the in-process scope) nor a terminal id (a
/// UUID) contains it, so the split back into `(scope, id)` is unambiguous.
const SCOPE_SEP: char = '\u{1f}';

fn doc_key(scope: &str, id: &str) -> String {
    format!("{scope}{SCOPE_SEP}{id}")
}

/// The Loro handles backing one terminal card, cached across applies so a tick
/// does not re-resolve containers by key. Valid until the key is deleted.
///
/// The scalar fields are cached so a tick only re-inserts a value that changed,
/// keeping idle terminals from producing a delta.
struct Slot {
    meta: LoroMap,
    screen: LoroText,
    alive: bool,
    cursor_row: i64,
    cursor_col: i64,
    cursor_visible: bool,
    cursor_shape: String,
    exit_code: Option<i32>,
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

    /// Reconcile the terminals under `scope` to exactly `frames`. Entries under
    /// other scopes are left alone. Returns the CRDT delta since the last
    /// broadcast when anything changed.
    fn apply_scope(&mut self, scope: &str, frames: &[TerminalFrame]) -> Result<Option<Vec<u8>>> {
        for frame in frames {
            let key = doc_key(scope, &frame.id);
            if !self.terminals.contains_key(&key) {
                let meta = self
                    .root
                    .insert_container(key.as_str(), LoroMap::new())
                    .map_err(loro_err)?;
                meta.insert("command", frame.command.as_str())
                    .map_err(loro_err)?;
                meta.insert("args", frame.args.as_str()).map_err(loro_err)?;
                meta.insert("rows", i64::from(frame.rows))
                    .map_err(loro_err)?;
                meta.insert("cols", i64::from(frame.cols))
                    .map_err(loro_err)?;
                meta.insert("alive", frame.alive).map_err(loro_err)?;
                write_cursor(&meta, frame)?;
                write_exit(&meta, frame)?;
                let screen = meta
                    .insert_container("screen", LoroText::new())
                    .map_err(loro_err)?;
                self.terminals.insert(
                    key.clone(),
                    Slot {
                        meta,
                        screen,
                        alive: frame.alive,
                        cursor_row: i64::from(frame.cursor_row),
                        cursor_col: i64::from(frame.cursor_col),
                        cursor_visible: frame.cursor_visible,
                        cursor_shape: frame.cursor_shape.clone(),
                        exit_code: frame.exit_code,
                    },
                );
            }

            let slot = self.terminals.get_mut(&key).expect("slot inserted above");
            if slot.alive != frame.alive {
                slot.meta.insert("alive", frame.alive).map_err(loro_err)?;
                slot.alive = frame.alive;
            }
            let cursor_row = i64::from(frame.cursor_row);
            let cursor_col = i64::from(frame.cursor_col);
            if slot.cursor_row != cursor_row
                || slot.cursor_col != cursor_col
                || slot.cursor_visible != frame.cursor_visible
                || slot.cursor_shape != frame.cursor_shape
            {
                write_cursor(&slot.meta, frame)?;
                slot.cursor_row = cursor_row;
                slot.cursor_col = cursor_col;
                slot.cursor_visible = frame.cursor_visible;
                slot.cursor_shape.clone_from(&frame.cursor_shape);
            }
            if slot.exit_code != frame.exit_code {
                write_exit(&slot.meta, frame)?;
                slot.exit_code = frame.exit_code;
            }
            if slot.screen.to_string() != frame.screen {
                slot.screen
                    .update(&frame.screen, loro::UpdateOptions::default())
                    .map_err(|source| Error::Dashboard {
                        message: format!("text update: {source}"),
                    })?;
            }
        }

        let prefix = format!("{scope}{SCOPE_SEP}");
        let live: HashSet<String> = frames
            .iter()
            .map(|frame| doc_key(scope, &frame.id))
            .collect();
        let dead: Vec<String> = self
            .terminals
            .keys()
            .filter(|key| key.starts_with(&prefix) && !live.contains(*key))
            .cloned()
            .collect();
        self.drop_keys(&dead)?;

        self.commit_delta()
    }

    /// Drop every terminal under `scope` (its producer disconnected). Returns
    /// the delta when the scope held anything.
    fn remove_scope(&mut self, scope: &str) -> Result<Option<Vec<u8>>> {
        let prefix = format!("{scope}{SCOPE_SEP}");
        let dead: Vec<String> = self
            .terminals
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
            self.terminals.remove(key);
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

fn loro_err(source: impl std::fmt::Display) -> Error {
    Error::Dashboard {
        message: source.to_string(),
    }
}

/// Write a frame's cursor fields into its meta map. One owner for the cursor
/// keys so the insert and per-tick update paths cannot disagree.
fn write_cursor(meta: &LoroMap, frame: &TerminalFrame) -> Result<()> {
    meta.insert("cursor_row", i64::from(frame.cursor_row))
        .map_err(loro_err)?;
    meta.insert("cursor_col", i64::from(frame.cursor_col))
        .map_err(loro_err)?;
    meta.insert("cursor_visible", frame.cursor_visible)
        .map_err(loro_err)?;
    meta.insert("cursor_shape", frame.cursor_shape.as_str())
        .map_err(loro_err)
}

/// Write a frame's exit code into its meta map: an `i64` when the process exited
/// with a code, otherwise absent (still running, or signalled).
fn write_exit(meta: &LoroMap, frame: &TerminalFrame) -> Result<()> {
    frame.exit_code.map_or_else(
        // A signalled or still-running process has no code; clear any prior
        // value so a re-spawned id under the same key never shows a stale exit
        // code. delete on an absent key is a harmless no-op.
        || meta.delete("exit_code").map_err(loro_err),
        |code| meta.insert("exit_code", i64::from(code)).map_err(loro_err),
    )
}

/// Owns the shared document and fans CRDT updates out to SSE subscribers.
///
/// One hub backs any number of frame sources. The in-process dashboard drives
/// it from a poll loop over a `TuiManager`; the aggregator drives it from many
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

    /// Reconcile the terminals under `scope` to exactly `frames` and broadcast
    /// the resulting delta. A failed apply is dropped: the next tick re-renders.
    pub fn apply_scope(&self, scope: &str, frames: &[TerminalFrame]) {
        let delta = self.state.lock().apply_scope(scope, frames);
        self.broadcast(delta);
    }

    /// Drop every terminal under `scope` and broadcast the delta.
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

    fn frame(id: &str, screen: &str) -> TerminalFrame {
        TerminalFrame {
            id: id.to_owned(),
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
        }
    }

    /// The core multi-producer invariant: one producer's reconcile never
    /// touches another's terminals, and dropping a producer removes only its
    /// own. Without scoping, the in-process `sync` deleted every key not in the
    /// frame list, which would erase every other producer.
    #[test]
    fn scopes_do_not_clobber_each_other() {
        let mut state = DocState::new();
        state
            .apply_scope("a", &[frame("1", "x"), frame("2", "y")])
            .unwrap();
        state.apply_scope("b", &[frame("3", "z")]).unwrap();
        assert_eq!(state.terminals.len(), 3);

        // Reconciling scope a to a single terminal drops a's other terminal and
        // leaves scope b alone.
        state.apply_scope("a", &[frame("1", "x")]).unwrap();
        assert_eq!(state.terminals.len(), 2);
        assert!(state.terminals.keys().any(|key| key.starts_with("b\u{1f}")));

        // Disconnecting producer a removes only a's terminals.
        state.remove_scope("a").unwrap();
        assert_eq!(state.terminals.len(), 1);
        assert!(state.terminals.keys().all(|key| key.starts_with("b\u{1f}")));
    }

    /// A tick that changes nothing produces no delta, so idle producers do not
    /// spam every connected browser.
    #[test]
    fn unchanged_apply_yields_no_delta() {
        let mut state = DocState::new();
        assert!(state.apply_scope("a", &[frame("1", "x")]).unwrap().is_some());
        assert!(state.apply_scope("a", &[frame("1", "x")]).unwrap().is_none());
        // A screen change does produce a delta.
        assert!(state.apply_scope("a", &[frame("1", "y")]).unwrap().is_some());
    }

    /// Removing a scope that holds nothing is a no-op, not a spurious broadcast.
    #[test]
    fn removing_empty_scope_yields_no_delta() {
        let mut state = DocState::new();
        assert!(state.remove_scope("ghost").unwrap().is_none());
    }
}
