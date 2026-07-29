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
//! # Who may write what, when several writers share a pane
//!
//! The two halves of a pane merge differently, and this is the one place the
//! CRDT does not save a careless writer:
//!
//! - a [`LoroText`] field (a body, an execution's `stdout`) **merges**, so two
//!   writers appending concurrently both survive;
//! - the `meta` [`LoroMap`] scalars (`title`, `kind`, `subtitle`) are
//!   **last-write-wins**, so two writers setting `title` concurrently silently
//!   lose one of them.
//!
//! So the convention is: **the writer that created a pane owns its scalars, and
//! anyone may append to its text.** Nothing here enforces that, deliberately.
//! Enforcing single ownership would make the case this document exists for --
//! several agents contributing competing findings about one subject --
//! impossible to express, and it would turn a merge into an error the writer
//! has to handle, which is what a CRDT is for avoiding.
//!
//! Leaving it unenforced is defensible only because a violation is *visible*:
//! [`LoroMap::get_last_editor`] reports who last set a key, so a title that
//! changed hands says so on the pane itself rather than in a log nobody opens.
//! See `map_scalars_report_their_last_editor` in `tests/peer_switching.rs`,
//! which is that convention written as a test.
//!
//! Every commit carries a millisecond wall-clock timestamp
//! ([`set_next_commit_timestamp`](LoroDoc::set_next_commit_timestamp)), and each
//! pane is stamped with a `created_at` the first time it appears. Together they
//! let a browser scrub the document to any past moment and show each resource's
//! age, with no producer opting in.
//!
//! Two more root containers hang off the same document, each with a different
//! owner. `__peers` maps a peer id to the [`Actor`] behind it, so a change reads
//! as "the agent" or "Ada" rather than as a random `u64`. `inputs` holds viewer
//! answers and is the one surface no producer writes: it is how a browser talks
//! back. Every commit also carries a JSON tag as its Loro commit message, which
//! is both what a history UI reads to label a change ([`Hub::history`]) and what
//! keeps a session's edits from collapsing into one change.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use loro::{
    Container, ExportMode, JsonMapOp, JsonOp, JsonOpContent, JsonTextOp, LoroDoc, LoroMap,
    LoroText, LoroValue, PeerID, Subscription as LoroSubscription, ValueOrContainer,
    VersionVector,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
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

/// Root map from peer id to the [`Actor`] behind it, keyed by the id in decimal
/// because a Loro peer id is a `u64` and a JSON number is not.
const PEERS_ROOT: &str = "__peers";

/// Root map holding viewer answers, keyed `"<scope>\x1f<pane>\x1f<field>"`.
const INPUTS_ROOT: &str = "inputs";

fn doc_key(scope: &str, id: &str) -> String {
    format!("{scope}{SCOPE_SEP}{id}")
}

/// The pane id half of a root-map key, for naming what a commit touched.
fn pane_id(key: &str) -> &str {
    key.rsplit(SCOPE_SEP).next().unwrap_or(key)
}

/// The key one viewer answer lives under in [`INPUTS_ROOT`].
fn input_key(scope: &str, pane: &str, field: &str) -> String {
    format!("{scope}{SCOPE_SEP}{pane}{SCOPE_SEP}{field}")
}

/// Split an input key back into its three parts, or `None` when it is not one.
///
/// A viewer writing a key of its own invention is not an error the hub can
/// answer -- the write already merged -- so an unparseable key is skipped by
/// [`DocState::inputs`] rather than guessed at.
fn split_input_key(key: &str) -> Option<InputParts<'_>> {
    let mut parts = key.split(SCOPE_SEP);
    let parsed = InputParts {
        scope: parts.next()?,
        pane: parts.next()?,
        field: parts.next()?,
    };
    parts.next().is_none().then_some(parsed)
}

/// The three parts of an input key.
struct InputParts<'a> {
    scope: &'a str,
    pane: &'a str,
    field: &'a str,
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

/// One scalar meta field a view contributes: the meta key and the value to
/// reconcile under it.
struct ScalarField {
    /// The meta key this scalar is written under. Fixed per view kind.
    name: &'static str,
    /// The value to reconcile, including [`Scalar::Absent`] to clear the key.
    value: Scalar,
}

/// The scalar meta fields a view contributes, besides the common `kind`,
/// `created_at`, `title`, and `subtitle` that every pane carries.
///
/// The keys a view returns are fixed for its kind, so a slot created for one
/// kind always sees the same set on every later tick.
fn view_scalars(view: &View) -> Vec<ScalarField> {
    let field = |name, value| ScalarField { name, value };
    match view {
        View::Terminal(t) => vec![
            field("rows", Scalar::Int(i64::from(t.rows))),
            field("cols", Scalar::Int(i64::from(t.cols))),
            field("alive", Scalar::Bool(t.alive)),
            field("cursor_row", Scalar::Int(i64::from(t.cursor_row))),
            field("cursor_col", Scalar::Int(i64::from(t.cursor_col))),
            field("cursor_visible", Scalar::Bool(t.cursor_visible)),
            field("cursor_shape", Scalar::Str(t.cursor_shape.clone())),
            field(
                "exit_code",
                t.exit_code
                    .map_or(Scalar::Absent, |code| Scalar::Int(i64::from(code))),
            ),
        ],
        View::Html(_) => Vec::new(),
        View::Exec(e) => vec![
            field("lang", Scalar::Str(e.lang.clone())),
            field("running", Scalar::Bool(e.running)),
            field("ok", e.ok.map_or(Scalar::Absent, Scalar::Bool)),
            field("topic", e.topic.clone().map_or(Scalar::Absent, Scalar::Str)),
            field(
                "duration_ms",
                // Saturating is deliberate and unreachable: a duration is
                // milliseconds since the exec started, and i64::MAX ms is ~292
                // million years. If it somehow were reached, a pane showing a
                // clamped duration still beats one showing none.
                #[allow(
                    clippy::fallible_int_fallback,
                    reason = "u64 milliseconds cannot exceed i64::MAX in any real exec"
                )]
                e.duration_ms.map_or(Scalar::Absent, |ms| {
                    Scalar::Int(i64::try_from(ms).unwrap_or(i64::MAX))
                }),
            ),
            field(
                "line",
                e.line
                    .map_or(Scalar::Absent, |line| Scalar::Int(i64::from(line))),
            ),
            field(
                "error_line",
                e.error_line
                    .map_or(Scalar::Absent, |line| Scalar::Int(i64::from(line))),
            ),
        ],
        View::Data(d) => vec![field("renderer", Scalar::Str(d.renderer.clone()))],
    }
}

/// One large mutable text field a view contributes: the container key and the
/// text to reconcile into it.
struct TextField {
    /// The text container key this field is stored under. Fixed per view kind.
    name: &'static str,
    /// The full text to reconcile into the container.
    value: String,
}

/// The large mutable text fields a view contributes, each stored in its own Loro
/// text container so it diffs and replays independently. The terminal's command
/// and args are not here: they ride in the pane's title and subtitle, which
/// every pane already has.
fn view_texts(view: &View) -> Vec<TextField> {
    let field = |name, value| TextField { name, value };
    match view {
        View::Terminal(t) => vec![
            field("body", t.screen.clone()),
            field("scrollback", t.scrollback.clone()),
        ],
        View::Html(h) => vec![field("body", h.html.clone())],
        View::Exec(e) => vec![
            field("source", e.source.clone()),
            field("stdout", e.stdout.clone()),
            field("stderr", e.stderr.clone()),
            field("result", e.result.clone()),
            // Inline-trace output→line map, canonicalized to JSON text (like the
            // data view's body) so it diffs and replays; the frontend parses it.
            field("trace", serde_json::to_string(&e.trace).unwrap_or_default()),
        ],
        // A data view's JSON is canonicalized to text so it diffs and replays
        // like any other body; the frontend parses it back.
        View::Data(d) => vec![field(
            "body",
            serde_json::to_string(&d.data).unwrap_or_default(),
        )],
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
    parent: Option<String>,
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
    /// Peer id (decimal) to actor. See [`PEERS_ROOT`].
    peers: LoroMap,
    /// Viewer answers. See [`INPUTS_ROOT`].
    inputs: LoroMap,
    panes: HashMap<String, Slot>,
    streamed: VersionVector,
    /// What this hub calls itself, shared with the first-commit callback because
    /// that callback runs while the `DocState` lock is held and so cannot read
    /// it back through `self`.
    identity: Arc<Mutex<Actor>>,
    /// Distinguishes one commit's message from the next. See [`CommitTag`].
    seq: u64,
    /// Held only for its `Drop`: a dropped `Subscription` unsubscribes, and an
    /// unregistered callback would leave every change unattributed.
    _attribution: LoroSubscription,
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
        let peers = doc.get_map(PEERS_ROOT);
        let inputs = doc.get_map(INPUTS_ROOT);
        let streamed = doc.oplog_vv();
        let identity = Arc::new(Mutex::new(Actor::default()));

        // Loro fires this once per peer that commits *locally*, and a write made
        // inside the callback joins that same commit -- so a peer can never
        // appear in the history before it has said who it is. A remote peer is
        // introduced by its own document the same way and arrives with the
        // merge; the hub does not guess on anyone else's behalf.
        let attribution = {
            let peers = peers.clone();
            let identity = Arc::clone(&identity);
            doc.subscribe_first_commit_from_peer(Box::new(move |payload| {
                let actor = identity.lock().clone();
                // A Loro callback has no channel to report through. The only way
                // this write fails is a detached document, which the hub never
                // does; if it ever did, the cost is an unlabelled peer id in the
                // history rather than a lost pane.
                let _ = write_actor(&peers, payload.peer, &actor);
                true
            }))
        };

        Self {
            doc,
            root,
            peers,
            inputs,
            panes: HashMap::new(),
            streamed,
            identity,
            seq: 0,
            _attribution: attribution,
        }
    }

    /// Record `actor` as this hub's identity and write it against the current
    /// peer id straight away, so an owner that declares before its first pane is
    /// still in the document, and a re-declaration takes effect at once.
    fn declare_identity(&mut self, actor: &Actor) -> Result<Option<Delta>> {
        // The guard has to be gone before the commit below: the first-commit
        // callback takes the same lock, and it is not reentrant.
        *self.identity.lock() = actor.clone();
        let peer = self.doc.peer_id().to_string();
        write_actor(&self.peers, self.doc.peer_id(), actor)?;
        self.commit_delta(CommitTag {
            on: "peers",
            add: vec![&peer],
            ..CommitTag::default()
        })
    }

    /// Every peer that has introduced itself in this document.
    fn actors(&self) -> HashMap<u64, Actor> {
        let mut actors = HashMap::new();
        self.peers.for_each(|key, value| {
            let (Ok(peer), Some(actor)) = (key.parse::<u64>(), read_actor(&value)) else {
                return;
            };
            actors.insert(peer, actor);
        });
        actors
    }

    /// Create the text container for a note field if it is not there yet.
    ///
    /// `ensure_mergeable_text` rather than `insert_container`: a plain child
    /// container gets an op-derived id, so two viewers creating one at the same
    /// key concurrently produce two containers and the map keeps exactly one --
    /// discarding everything the loser typed. A mergeable child has a key-derived
    /// id, so concurrent creation converges on one container and both viewers'
    /// sentences survive. Declaring it here as well means the container is
    /// already in the snapshot every viewer imports, so no viewer has to create
    /// it at all.
    fn declare_note(&mut self, scope: &str, pane: &str, field: &str) -> Result<Option<Delta>> {
        let key = input_key(scope, pane, field);
        self.inputs.ensure_mergeable_text(&key).map_err(loro_err)?;
        self.commit_delta(CommitTag {
            on: "inputs",
            scope: Some(scope),
            pane: Some(pane),
            add: vec![field],
            ..CommitTag::default()
        })
    }

    /// The answer to a single-answer field, or `None` when nobody has answered.
    fn choice(&self, scope: &str, pane: &str, field: &str) -> Option<String> {
        match self.inputs.get(&input_key(scope, pane, field))? {
            ValueOrContainer::Value(value) => scalar_answer(&value),
            ValueOrContainer::Container(_) => None,
        }
    }

    /// The text of a note field, or `None` when the field was never declared.
    /// An empty string means declared and untouched, which is a different thing.
    fn note(&self, scope: &str, pane: &str, field: &str) -> Option<String> {
        match self.inputs.get(&input_key(scope, pane, field))? {
            ValueOrContainer::Container(Container::Text(text)) => Some(text.to_string()),
            _ => None,
        }
    }

    /// Every input in the document, sorted so two reads of an unchanged document
    /// agree (Loro's map iteration order is its own business).
    fn inputs(&self) -> Vec<InputEntry> {
        let mut entries = Vec::new();
        self.inputs.for_each(|key, value| {
            let Some(parts) = split_input_key(key) else {
                return;
            };
            let input = match value {
                ValueOrContainer::Container(Container::Text(text)) => Input::Note {
                    text: text.to_string(),
                },
                ValueOrContainer::Value(value) => match scalar_answer(&value) {
                    Some(value) => Input::Choice { value },
                    None => return,
                },
                ValueOrContainer::Container(_) => return,
            };
            entries.push(InputEntry {
                scope: parts.scope.to_owned(),
                pane: parts.pane.to_owned(),
                field: parts.field.to_owned(),
                value: input,
            });
        });
        entries.sort_by(|left, right| {
            (&left.scope, &left.pane, &left.field).cmp(&(&right.scope, &right.pane, &right.field))
        });
        entries
    }

    /// The whole oplog as changes, oldest first, each attributed to the actor its
    /// peer declared.
    ///
    /// `export_json_updates_without_peer_compression` rather than the compressed
    /// form: the compressed one replaces peer ids with indices into a side table,
    /// which a consumer then has to join back before it can attribute anything.
    fn history(&self) -> Vec<HistoryChange> {
        let actors = self.actors();
        let updates = self.doc.export_json_updates_without_peer_compression(
            &VersionVector::default(),
            &self.doc.oplog_vv(),
        );
        updates
            .changes
            .into_iter()
            .map(|change| HistoryChange {
                peer: change.id.peer.to_string(),
                counter: change.id.counter,
                lamport: change.lamport,
                timestamp: change.timestamp,
                message: change.msg,
                actor: actors.get(&change.id.peer).cloned(),
                deps: change
                    .deps
                    .iter()
                    .map(|dep| format!("{}@{}", dep.peer, dep.counter))
                    .collect(),
                ops: change.ops.iter().map(history_op).collect(),
            })
            .collect()
    }

    /// Reconcile the panes under `scope` to exactly `panes`. Entries under other
    /// scopes are left alone. Returns the CRDT delta since the last broadcast
    /// when anything changed.
    fn apply_scope(&mut self, scope: &str, panes: &[Pane]) -> Result<Option<Delta>> {
        let mut added: Vec<&str> = Vec::new();
        let mut changed: Vec<&str> = Vec::new();
        for pane in panes {
            let key = doc_key(scope, &pane.id);
            let kind = pane.view.kind();

            // A reused id whose kind changed cannot be edited in place: the
            // stored scalars and text fields mean something different now. Drop
            // the old entry so the create path below rebuilds it cleanly.
            if self.panes.get(&key).is_some_and(|slot| slot.kind != kind) {
                self.drop_keys(std::slice::from_ref(&key))?;
            }

            let fresh = !self.panes.contains_key(&key);
            if fresh {
                self.create_slot(&key, pane)?;
                added.push(&pane.id);
            }
            // A created pane's first values are part of creating it, so listing
            // it twice would only pad the commit tag.
            if self.update_slot(&key, pane)? && !fresh {
                changed.push(&pane.id);
            }
        }

        let prefix = format!("{scope}{SCOPE_SEP}");
        let live: HashSet<String> = panes.iter().map(|p| doc_key(scope, &p.id)).collect();
        let dead: Vec<String> = self
            .panes
            .keys()
            .filter(|key| key.starts_with(&prefix) && !live.contains(*key))
            .cloned()
            .collect();
        let dropped: Vec<&str> = dead.iter().map(|key| pane_id(key)).collect();
        self.drop_keys(&dead)?;

        self.commit_delta(CommitTag {
            on: "panes",
            scope: Some(scope),
            add: added,
            set: changed,
            del: dropped,
            ..CommitTag::default()
        })
    }

    /// Create or update exactly one pane, leaving every other entry untouched.
    ///
    /// The per-pane twin of [`apply_scope`](Self::apply_scope), and the right
    /// call when several writers share one document: `apply_scope` reconciles a
    /// whole scope to the slice it is handed, so a writer that owns three panes
    /// has to re-send all three to change one, and has to hold a copy of them to
    /// do it. Naming the pane makes "you can only touch what you name"
    /// structural rather than a convention, and costs one pane per publish.
    ///
    /// The key is the pane id with no scope prefix. Scoped and unscoped entries
    /// coexist in the same root map; readers already treat a key with no
    /// separator as unscoped.
    fn set_pane(&mut self, pane: &Pane) -> Result<Option<Delta>> {
        let key = pane.id.clone();
        let kind = pane.view.kind();

        // Same reasoning as `apply_scope`: a reused id whose kind changed cannot
        // be edited in place, because the stored fields mean something else now.
        if self.panes.get(&key).is_some_and(|slot| slot.kind != kind) {
            self.drop_keys(std::slice::from_ref(&key))?;
        }

        let fresh = !self.panes.contains_key(&key);
        if fresh {
            self.create_slot(&key, pane)?;
        }
        let changed = self.update_slot(&key, pane)?;

        self.commit_delta(CommitTag {
            on: "panes",
            pane: Some(&pane.id),
            add: if fresh { vec![pane.id.as_str()] } else { Vec::new() },
            set: if changed && !fresh {
                vec![pane.id.as_str()]
            } else {
                Vec::new()
            },
            ..CommitTag::default()
        })
    }

    /// Remove one pane by id. Unknown ids are a no-op and broadcast nothing, so
    /// a writer retiring something twice does not wake every viewer twice.
    fn drop_pane(&mut self, id: &str) -> Result<Option<Delta>> {
        let key = id.to_owned();
        if !self.panes.contains_key(&key) {
            return Ok(None);
        }
        self.drop_keys(std::slice::from_ref(&key))?;
        self.commit_delta(CommitTag {
            on: "panes",
            pane: Some(id),
            del: vec![id],
            ..CommitTag::default()
        })
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
        for field in view_texts(&pane.view) {
            let text = meta
                .insert_container(field.name, LoroText::new())
                .map_err(loro_err)?;
            // A fresh container is empty, so a cached empty value matches it and
            // the first update writes only a non-empty initial body.
            texts.push(TextSlot {
                key: field.name,
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
                parent: None,
                scalars: HashMap::new(),
                texts,
            },
        );
        Ok(())
    }

    /// Reconcile one existing pane's scalars and text fields to `pane`, writing
    /// only the values that changed so an idle pane produces no delta. Reports
    /// whether anything was written, which is what the commit tag names.
    #[allow(
        clippy::useless_let_if_seq,
        reason = "a chain of independent field reconciliations, not one if/else expression"
    )]
    fn update_slot(&mut self, key: &str, pane: &Pane) -> Result<bool> {
        let slot = self.panes.get_mut(key).expect("slot exists");
        let mut wrote = false;
        if slot.title != pane.title {
            slot.meta
                .insert("title", pane.title.as_str())
                .map_err(loro_err)?;
            slot.title.clone_from(&pane.title);
            wrote = true;
        }
        if slot.subtitle != pane.subtitle {
            slot.meta
                .insert("subtitle", pane.subtitle.as_str())
                .map_err(loro_err)?;
            slot.subtitle.clone_from(&pane.subtitle);
            wrote = true;
        }
        if slot.parent != pane.parent {
            match &pane.parent {
                Some(parent) => slot
                    .meta
                    .insert("parent", parent.as_str())
                    .map_err(loro_err)?,
                None => slot.meta.delete("parent").map_err(loro_err)?,
            }
            slot.parent.clone_from(&pane.parent);
            wrote = true;
        }
        for field in view_scalars(&pane.view) {
            if slot.scalars.get(field.name) != Some(&field.value) {
                write_scalar(&slot.meta, field.name, &field.value)?;
                slot.scalars.insert(field.name, field.value);
                wrote = true;
            }
        }
        for field in view_texts(&pane.view) {
            // Match by key rather than position: the key set is fixed per kind,
            // so the lookup always hits, and matching keeps the two projections
            // from silently drifting if one is reordered.
            if let Some(text_slot) = slot.texts.iter_mut().find(|slot| slot.key == field.name)
                && text_slot.value != field.value
            {
                set_text(&text_slot.text, &field.value)?;
                text_slot.value = field.value;
                wrote = true;
            }
        }
        Ok(wrote)
    }

    /// Keep panes under `scope` when its producer disconnects.
    ///
    /// A terminal/resource pane is still useful after the process or producer
    /// dies: it is the final state a human wants to inspect. The next live
    /// snapshot from the same scope still reconciles normally through
    /// [`apply_scope`](Self::apply_scope), including deleting panes that are no
    /// longer reported by that producer.
    #[allow(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        clippy::unnecessary_wraps,
        clippy::missing_const_for_fn,
        reason = "keeps the `&mut self -> Result<delta>` shape its sibling scope \
                  operations and callers share; narrowing it would make the one \
                  no-op in the set the odd one out"
    )]
    fn remove_scope(&mut self, scope: &str) -> Result<Option<Delta>> {
        let _ = scope;
        Ok(None)
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
    /// wall-clock timestamp so the browser timeline has a fine-grained axis, and
    /// with `tag` as its commit message so the change is legible and stays its
    /// own change.
    fn commit_delta(&mut self, tag: CommitTag<'_>) -> Result<Option<Delta>> {
        self.seq += 1;
        let tag = CommitTag {
            seq: self.seq,
            ..tag
        };
        let message = serde_json::to_string(&tag).map_err(loro_err)?;
        self.doc.set_next_commit_message(&message);
        self.doc.set_next_commit_timestamp(now_ms());
        self.doc.commit();
        let current = self.doc.oplog_vv();
        if current == self.streamed {
            return Ok(None);
        }
        let bytes = self
            .doc
            .export(ExportMode::updates(&self.streamed))
            .map_err(loro_err)?;
        // Who wrote this delta, for free: exactly the peers whose counter moved
        // between the last broadcast and now. Reading it here costs one pass
        // over a small map; recovering it later would mean decoding the update.
        let authors = current
            .iter()
            .filter(|(peer, counter)| {
                self.streamed.get(peer).copied().unwrap_or_default() < **counter
            })
            .map(|(peer, _)| *peer)
            .collect();
        self.streamed = current;
        Ok(Some(Delta { bytes, authors }))
    }

    /// A full snapshot of the current document, for a newly-connected client, a
    /// client that fell too far behind the update stream, or a persisted
    /// recording. Includes the complete oplog, so the receiver can replay any
    /// past version, not only the latest state.
    fn snapshot(&self) -> Result<Vec<u8>> {
        self.doc.export(ExportMode::Snapshot).map_err(loro_err)
    }

    /// Merge remote CRDT bytes into the document and export the delta, so every
    /// live client converges on the merged result.
    ///
    /// Reports whether Loro applied the update or only recorded it. A pending
    /// import is neither success nor failure: the ops sit in the oplog but not
    /// in the document state, so the edit is invisible until the range it
    /// depends on arrives.
    fn import(&mut self, bytes: &[u8]) -> Result<Imported> {
        let status = self.doc.import(bytes).map_err(loro_err)?;
        // A remote edit moves containers this side caches. Left stale, the next
        // `update_slot` compares a producer value against a belief that is no
        // longer true, and either skips a write that is needed or repeats one
        // that is not -- so refresh before anything reads them.
        self.resync_caches();
        // The merged changes carry their writers' own messages; this commit only
        // flushes anything local that was pending, and is usually empty.
        let delta = self.commit_delta(CommitTag {
            on: "import",
            ..CommitTag::default()
        })?;
        let merge = if status.pending.is_some() {
            Merge::Pending
        } else {
            Merge::Applied
        };
        Ok(Imported { merge, delta })
    }

    /// Refresh every write-through cache from the container it shadows.
    ///
    /// Scoped to the panes this side already tracks. Producers own the pane set
    /// and remotes edit values inside it, so a remote that adds or removes a
    /// pane entry is outside the model; a handle whose container a remote
    /// deleted fails loudly on its next write rather than silently no-oping.
    fn resync_caches(&mut self) {
        for slot in self.panes.values_mut() {
            // An absent key resyncs to the sentinel, not to empty: empty is a
            // value a producer can legitimately hold, and matching it would
            // suppress the write that recreates the key.
            slot.title = read_str(&slot.meta, "title").unwrap_or_else(sentinel);
            slot.subtitle = read_str(&slot.meta, "subtitle").unwrap_or_else(sentinel);
            slot.parent = read_str(&slot.meta, "parent");
            let names: Vec<&'static str> = slot.scalars.keys().copied().collect();
            for name in names {
                let fresh = read_scalar(&slot.meta, name);
                slot.scalars.insert(name, fresh);
            }
            for text in &mut slot.texts {
                text.value = text.text.to_string();
            }
        }
    }
}

/// The commit message every hub commit carries, as JSON.
///
/// Two jobs. It tells a history UI what a change was for without decoding its
/// ops. And it keeps that change separate at all: Loro's change-merge interval
/// defaults to 1000 *seconds*, so consecutive local commits fold into one change
/// unless something about them differs, and the commit message is one of the
/// fields that comparison covers. `seq` is therefore not decoration -- without a
/// field that differs every time, a whole session of pane edits arrives at the
/// history UI as a single change.
#[derive(Default, Serialize)]
struct CommitTag<'a> {
    /// What the commit is: `panes`, `inputs`, `peers`, or `import`.
    on: &'static str,
    /// Monotonic per hub. Gaps are commits that turned out to write nothing.
    seq: u64,
    /// The producer scope, when the commit belongs to one.
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<&'a str>,
    /// The pane the commit is about, when it is about exactly one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pane: Option<&'a str>,
    /// Keys this commit created.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    add: Vec<&'a str>,
    /// Keys whose values this commit rewrote.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    set: Vec<&'a str>,
    /// Keys this commit removed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    del: Vec<&'a str>,
}

/// Write one peer's actor entry.
///
/// `ensure_mergeable_map` so the entry converges even in the one case that could
/// collide -- two documents that drew the same random peer id -- instead of one
/// of them shadowing the other's label.
fn write_actor(peers: &LoroMap, peer: u64, actor: &Actor) -> Result<()> {
    let entry = peers
        .ensure_mergeable_map(&peer.to_string())
        .map_err(loro_err)?;
    entry.insert("kind", actor.kind.tag()).map_err(loro_err)?;
    entry
        .insert("label", actor.label.as_str())
        .map_err(loro_err)?;
    Ok(())
}

/// Read one peer's actor entry back, or `None` when it is missing the fields a
/// reader needs -- an entry written by a peer that speaks a later dialect.
fn read_actor(entry: &ValueOrContainer) -> Option<Actor> {
    let ValueOrContainer::Container(Container::Map(entry)) = entry else {
        return None;
    };
    Some(Actor {
        kind: ActorKind::from_tag(&read_str(entry, "kind")?)?,
        label: read_str(entry, "label")?,
    })
}

/// One viewer answer as a string, whatever scalar spelling it arrived in: a
/// radio group posts `"approve"` and a checkbox posts `true`, and a producer
/// branching on the answer wants one type rather than three. A container is not
/// a single answer, so it is not one of these.
fn scalar_answer(value: &LoroValue) -> Option<String> {
    match value {
        LoroValue::String(text) => Some(text.to_string()),
        LoroValue::Bool(flag) => Some(flag.to_string()),
        LoroValue::I64(int) => Some(int.to_string()),
        _ => None,
    }
}

/// Summarise one JSON op for the history surface.
fn history_op(op: &JsonOp) -> HistoryOp {
    let container = op.container.to_string();
    match &op.content {
        JsonOpContent::Map(JsonMapOp::Insert { key, .. }) => HistoryOp::MapSet {
            container,
            key: key.clone(),
        },
        JsonOpContent::Map(JsonMapOp::Delete { key }) => HistoryOp::MapDelete {
            container,
            key: key.clone(),
        },
        JsonOpContent::Text(JsonTextOp::Insert { text, .. }) => HistoryOp::TextInsert {
            container,
            chars: text.chars().count(),
        },
        JsonOpContent::Text(JsonTextOp::Delete { len, start_id, .. }) => HistoryOp::TextDelete {
            container,
            // A backwards delete is spelled with a negative length.
            chars: len.unsigned_abs() as usize,
            start: format!("{}@{}", start_id.peer, start_id.counter),
        },
        _ => HistoryOp::Other { container },
    }
}

/// Read one scalar meta value back out of a pane's map.
///
/// The inverse of [`write_scalar`], and it exists for [`DocState::import`]:
/// after a remote merge the write-through caches hold this side's stale belief,
/// and the only truth about what a field now contains is the container itself. A
/// key that is absent, or holds a type no view writes, reads as
/// [`Scalar::Absent`] -- the same value that means "ensure it is not present",
/// so a resync followed by a producer tick converges either way.
fn read_scalar(meta: &LoroMap, name: &str) -> Scalar {
    match meta.get(name).map(|value| value.get_deep_value()) {
        Some(LoroValue::Bool(flag)) => Scalar::Bool(flag),
        Some(LoroValue::I64(int)) => Scalar::Int(int),
        Some(LoroValue::String(text)) => Scalar::Str(text.to_string()),
        _ => Scalar::Absent,
    }
}

/// Read one string meta value, or `None` when absent or holding another type.
fn read_str(meta: &LoroMap, name: &str) -> Option<String> {
    match meta.get(name).map(|value| value.get_deep_value()) {
        Some(LoroValue::String(text)) => Some(text.to_string()),
        _ => None,
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

/// What one [`DocState::import`] did: whether Loro applied the update or only
/// recorded it, and the delta to broadcast (absent when nothing was pending).
struct Imported {
    merge: Merge,
    delta: Option<Delta>,
}

/// A committed delta and the peers whose ops are in it.
///
/// The author set is taken from the version-vector diff at commit time, so it
/// costs nothing beyond a pass over a map, and it is what lets a subscriber tell
/// its own writes from someone else's without decoding the update.
struct Delta {
    bytes: Vec<u8>,
    authors: Vec<PeerID>,
}

/// One broadcast delta in both of the forms its transports need.
///
/// SSE is a text protocol and has to base64 its payload; the Loro websocket
/// protocol frames the same bytes raw. Carrying both means the encode happens
/// once at fan-in rather than once per subscriber, and one channel keeps one
/// lag domain -- two channels would let the same delta sit at different
/// positions in each, so a subscriber resyncing on one could double-apply
/// from the other.
pub struct Update {
    /// The raw Loro update, as the websocket transport frames it.
    pub(crate) bytes: Vec<u8>,
    /// The same bytes base64'd, as an SSE `data:` field requires.
    pub(crate) encoded: String,
    /// Peers whose ops are in this delta. See [`Update::authors`].
    authors: Vec<PeerID>,
}

impl Update {
    /// The raw Loro update, to import into a replica of this document.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Peers whose ops are in this delta.
    ///
    /// This is how a subscriber that also writes tells its own changes from
    /// everyone else's. Comparing against [`Hub::peer_id`] classifies a delta as
    /// this hub's own write or as a merge from outside; where the hub writes for
    /// several agents under a peer each, the set names exactly which of them
    /// wrote, so a wake can skip the agent that caused it.
    #[must_use]
    pub fn authors(&self) -> &[PeerID] {
        &self.authors
    }

    /// True when every op in this delta came from `peer`, i.e. the delta is that
    /// peer's own echo and a subscriber acting for it has nothing new to learn.
    #[must_use]
    pub fn is_only_from(&self, peer: PeerID) -> bool {
        self.authors == [peer]
    }
}

/// A new subscriber's starting point: the current full snapshot taken under the
/// hub lock, plus the live update stream whose first item lines up with it.
pub struct Subscription {
    /// The full document snapshot at subscribe time, for the client to import
    /// before applying any update.
    pub(crate) snapshot: Vec<u8>,
    /// The CRDT update stream, consistent with `snapshot`.
    pub(crate) updates: broadcast::Receiver<Arc<Update>>,
}

impl Subscription {
    /// Split into the seed snapshot and the update stream that continues it.
    ///
    /// Import the snapshot before applying anything off the receiver: the two
    /// were taken under one lock, so together they are a gap-free view, and in
    /// the other order the first updates are applied to an empty document.
    #[must_use]
    pub fn into_parts(self) -> (Vec<u8>, broadcast::Receiver<Arc<Update>>) {
        (self.snapshot, self.updates)
    }
}

/// Who is behind a peer id.
///
/// A Loro peer id is a random `u64` per document and the library forbids pinning
/// one per actor without a lock, so the id alone says nothing about who made a
/// change. Instead each participant writes its own `__peers` entry on its first
/// local commit, the one moment it can speak for itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    /// Software or a person -- the first thing a reviewer of a change asks.
    pub kind: ActorKind,
    /// Display label: an agent's run name, a human's handle.
    pub label: String,
}

impl Actor {
    /// An agent-driven writer.
    #[must_use]
    pub fn agent(label: impl Into<String>) -> Self {
        Self {
            kind: ActorKind::Agent,
            label: label.into(),
        }
    }

    /// A person at a browser.
    #[must_use]
    pub fn human(label: impl Into<String>) -> Self {
        Self {
            kind: ActorKind::Human,
            label: label.into(),
        }
    }
}

impl Default for Actor {
    /// What the hub calls itself until its owner says otherwise
    /// ([`Hub::declare_identity`]). Every frame source on the producer side is a
    /// program, so guessing `Human` here would be wrong far more often than not.
    fn default() -> Self {
        Self::agent("producer")
    }
}

/// Which side of the collaboration a peer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorKind {
    /// A program: a producer, an agent run, the hub itself.
    Agent,
    /// A person editing through a browser.
    Human,
}

impl ActorKind {
    /// The spelling stored in the document and on the wire.
    const fn tag(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Human => "human",
        }
    }

    /// The inverse of [`tag`](Self::tag); `None` for a spelling this build does
    /// not know, which is how a newer peer's kind arrives.
    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "agent" => Some(Self::Agent),
            "human" => Some(Self::Human),
            _ => None,
        }
    }
}

/// One viewer answer, tagged with the merge semantics it was stored under.
///
/// The distinction is the whole point of the type. Putting free text on a map
/// key would resolve two people typing at once by keeping one sentence and
/// discarding the other, silently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "shape", rename_all = "lowercase")]
pub enum Input {
    /// A single-answer field (a verdict, an approval) on a map key, so the last
    /// write wins -- which is what "one answer" means.
    Choice {
        /// The answer as written, whatever scalar the viewer sent.
        value: String,
    },
    /// Free text in its own text container, so two viewers' edits both survive.
    Note {
        /// The merged text.
        text: String,
    },
}

/// One input the document holds, with the pane it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputEntry {
    /// The producer scope the pane belongs to.
    pub scope: String,
    /// The pane id, as the producer spelled it.
    pub pane: String,
    /// The field name within that pane.
    pub field: String,
    /// The answer, and how it merges.
    pub value: Input,
}

/// One change in the document's history, with its writer resolved to an actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistoryChange {
    /// The writing peer in decimal, the same spelling as the `__peers` key,
    /// because a `u64` does not survive a JSON number.
    pub peer: String,
    /// Counter of the change's first op; `"<peer>@<counter>"` is its id.
    pub counter: i32,
    /// Logical clock, for ordering changes that are concurrent in wall time.
    pub lamport: u32,
    /// Wall clock the writer stamped, in milliseconds. Every commit through
    /// this hub is stamped; `DocState::commit_delta` is where.
    pub timestamp: i64,
    /// The commit tag, a JSON object for a change this hub wrote. Absent for a
    /// writer that set no message.
    pub message: Option<String>,
    /// Who the peer said it was, absent when it never introduced itself.
    pub actor: Option<Actor>,
    /// Change ids this one builds on, `"<peer>@<counter>"`, for the DAG.
    pub deps: Vec<String>,
    /// What the change did.
    pub ops: Vec<HistoryOp>,
}

/// One operation inside a change, summarised for display.
///
/// Deliberately not a transcription of Loro's JSON op. A text op's `pos` there
/// is an *entity* index -- style anchors take slots of their own -- so showing
/// it as a character offset would point at the wrong place in the body. The
/// length is meaningful and the position is not, so only the length is here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HistoryOp {
    /// A map key was written. The value is not copied here: it is in the
    /// document at this version, and a nested container's value would make one
    /// history row larger than the whole document.
    MapSet {
        /// The container id, `"cid:root-panes:Map"` and friends.
        container: String,
        /// The key written.
        key: String,
    },
    /// A map key was removed.
    MapDelete {
        /// The container id.
        container: String,
        /// The key removed.
        key: String,
    },
    /// Characters were inserted into a text container.
    TextInsert {
        /// The container id.
        container: String,
        /// How many characters, not where.
        chars: usize,
    },
    /// Characters were removed from a text container.
    TextDelete {
        /// The container id.
        container: String,
        /// How many characters.
        chars: usize,
        /// Id of the first character removed, `"<peer>@<counter>"`. The only
        /// stable handle on the deleted span, since the position is an entity
        /// index.
        start: String,
    },
    /// A list, tree, or richtext-mark op. Named by container so a history UI can
    /// still show that something happened, without this enum having to grow a
    /// variant per Loro container type.
    Other {
        /// The container id.
        container: String,
    },
}

/// What [`Hub::import`] did with an update.
///
/// Named rather than a bare `bool` because the two cases need different handling
/// by the caller and "false" does not say which is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Merge {
    /// Every op applied, and the delta has gone out to live clients.
    Applied,
    /// The update depends on ops this document does not have, so Loro recorded
    /// it in the oplog without applying it to the document state. The edit is
    /// invisible until the missing range arrives, so a caller must answer with
    /// the sender's missing history rather than treat this as done.
    Pending,
}

/// Owns the shared document and fans CRDT updates out to SSE subscribers.
///
/// One hub backs any number of frame sources. The in-process dashboard drives it
/// from a poll loop over a `TuiManager`; the aggregator drives it from many
/// unix-socket readers. Both call [`apply_scope`](Self::apply_scope) and
/// [`remove_scope`](Self::remove_scope); the hub serializes them under one lock.
pub struct Hub {
    state: Mutex<DocState>,
    updates: broadcast::Sender<Arc<Update>>,
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

    /// Create or update one pane, leaving every other entry untouched.
    ///
    /// Prefer this to [`apply_scope`](Self::apply_scope) when more than one
    /// writer shares the document: it touches only the pane it names, so a
    /// writer needs no copy of everything else it owns in order to change one
    /// thing. See the module docs for which parts of a pane merge and which do
    /// not.
    pub fn set_pane(&self, pane: &Pane) {
        let delta = self.state.lock().set_pane(pane);
        self.broadcast(delta);
    }

    /// Remove one pane by id. An unknown id is a no-op that broadcasts nothing.
    pub fn drop_pane(&self, id: &str) {
        let delta = self.state.lock().drop_pane(id);
        self.broadcast(delta);
    }

    /// Merge remote CRDT bytes into the shared document and broadcast the
    /// result, so every other connected client converges on it.
    ///
    /// This is the half that makes the document a CRDT rather than a codec:
    /// without it the hub has exactly one writer and a browser can only read.
    /// The sender is inside the broadcast set and receives its own ops back --
    /// harmless, because a Loro import is idempotent, and cheaper than tracking
    /// a version vector per subscriber to suppress the echo.
    /// # Errors
    ///
    /// Returns the Loro decode error when `bytes` is not a valid update or
    /// snapshot. A well-formed update whose dependencies have not arrived is
    /// not an error: it reports [`Merge::Pending`].
    pub fn import(&self, bytes: &[u8]) -> Result<Merge> {
        let Imported { merge, delta } = self.state.lock().import(bytes)?;
        self.broadcast(Ok(delta));
        Ok(merge)
    }

    /// Declare who this hub is, so its changes are attributed to a named actor
    /// instead of to a bare peer id.
    ///
    /// Takes effect for changes already committed too: the entry is one map key
    /// and the newest declaration wins, so an owner that learns its own name
    /// late can still say it.
    pub fn declare_identity(&self, actor: &Actor) {
        let delta = self.state.lock().declare_identity(actor);
        self.broadcast(delta);
    }

    /// This hub's own Loro peer id.
    ///
    /// Paired with [`Update::authors`] it is what lets an in-process consumer
    /// classify a delta as this hub's own write or as a merge from a browser or
    /// another peer, without decoding the update.
    #[must_use]
    pub fn peer_id(&self) -> PeerID {
        self.state.lock().doc.peer_id()
    }

    /// Every peer that has introduced itself, keyed by peer id.
    #[must_use]
    pub fn actors(&self) -> HashMap<u64, Actor> {
        self.state.lock().actors()
    }

    /// Declare a free-text input on a pane, creating its text container.
    ///
    /// Free text goes in a text container rather than on a map key because a map
    /// key is last-write-wins: two people typing into the same box would resolve
    /// to one of the two sentences, and the other would be gone without a trace.
    /// A single-answer field wants exactly that resolution and so needs no
    /// declaration -- a viewer writes the key directly, and the newest answer is
    /// the answer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dashboard`] when the key already holds something that is
    /// not a mergeable text container.
    pub fn declare_note(&self, scope: &str, pane: &str, field: &str) -> Result<()> {
        let delta = self.state.lock().declare_note(scope, pane, field)?;
        self.broadcast(Ok(delta));
        Ok(())
    }

    /// The answer to a single-answer field, or `None` if nobody has answered.
    ///
    /// A viewer writes it by inserting `"<scope>\x1f<pane>\x1f<field>"` into the
    /// root `inputs` map of its own document and posting the update to `/apply`.
    #[must_use]
    pub fn choice(&self, scope: &str, pane: &str, field: &str) -> Option<String> {
        self.state.lock().choice(scope, pane, field)
    }

    /// The text of a note field, or `None` when it was never declared. An empty
    /// string means declared and untouched, which is a different answer.
    #[must_use]
    pub fn note(&self, scope: &str, pane: &str, field: &str) -> Option<String> {
        self.state.lock().note(scope, pane, field)
    }

    /// Every viewer answer in the document, ordered by scope, pane, then field.
    #[must_use]
    pub fn inputs(&self) -> Vec<InputEntry> {
        self.state.lock().inputs()
    }

    /// The document's changes, oldest first, each with its commit tag and the
    /// actor its peer declared -- everything a history UI draws a timeline from.
    #[must_use]
    pub fn history(&self) -> Vec<HistoryChange> {
        self.state.lock().history()
    }

    fn broadcast(&self, delta: Result<Option<Delta>>) {
        if let Ok(Some(Delta { bytes, authors })) = delta {
            let encoded = BASE64.encode(&bytes);
            let _ = self.updates.send(Arc::new(Update {
                bytes,
                encoded,
                authors,
            }));
        }
    }

    /// Subscribe to the CRDT update stream and read the current full snapshot,
    /// both under one lock so the snapshot version lines up with the first
    /// update the subscriber will receive.
    ///
    /// This is the signal an in-process consumer needs to keep a replica of the
    /// document current: seed from the snapshot, then import each
    /// [`Update::bytes`]. Polling [`export_snapshot`](Self::export_snapshot) on
    /// a timer instead costs a whole-document export and import per tick and
    /// still cannot say who wrote anything (ENG-10199).
    pub fn subscribe(&self) -> Subscription {
        let state = self.state.lock();
        let updates = self.updates.subscribe();
        Subscription {
            snapshot: state.snapshot().unwrap_or_default(),
            updates,
        }
    }

    /// A receiver on the update stream alone.
    ///
    /// [`subscribe`](Self::subscribe) pairs a receiver with the snapshot it
    /// lines up with, which is what a client needs. The websocket relay is a
    /// fan-out middleman that seeds nobody -- each websocket client takes its
    /// own snapshot after subscribing to the relay -- so it wants the stream
    /// without paying for a snapshot export per hub delta.
    pub fn updates(&self) -> broadcast::Receiver<Arc<Update>> {
        self.updates.subscribe()
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
    use loro::{Container, ValueOrContainer};

    use crate::pane::{ExecTraceLine, ExecView, TerminalView};

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
                scrollback: String::new(),
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

    /// The CRDT half. A second peer's edit merges into the shared document
    /// instead of being refused, which is what `import` exists to make true.
    #[test]
    fn a_remote_edit_merges_into_the_shared_document() {
        let hub = Hub::new();
        hub.apply_scope("p", &[terminal("t1", "before")]);

        // A browser starts from the snapshot, as the real client does.
        let peer = LoroDoc::new();
        peer.import(&hub.export_snapshot())
            .expect("snapshot imports");
        // Inputs live in their own root container: producers own `panes` and
        // never write here, so an answer and a producer tick cannot collide.
        peer.get_map("inputs")
            .insert("t1.verdict", "splice")
            .expect("remote write");
        peer.commit();

        assert_eq!(
            hub.import(&peer.export(ExportMode::Snapshot).expect("export"))
                .expect("merge"),
            Merge::Applied
        );

        // Read back through a third peer, so the assertion crosses the wire
        // rather than reading the hub's own handles.
        let reader = LoroDoc::new();
        reader
            .import(&hub.export_snapshot())
            .expect("reader imports");
        let dump = format!("{:?}", reader.get_deep_value());
        assert!(
            dump.contains("splice"),
            "remote edit lost in the merge: {dump}"
        );
    }

    /// The write-through cache is this side's belief about the document, and a
    /// remote edit invalidates it. Without the resync the cache still matches
    /// the producer's value, the write is skipped, and the pane is stuck on the
    /// remote's text for as long as the producer keeps reporting the same thing.
    #[test]
    fn a_remote_body_edit_is_not_masked_by_the_write_through_cache() {
        let mut state = DocState::new();
        state
            .apply_scope("p", &[terminal("t1", "hello")])
            .expect("apply");
        let key = doc_key("p", "t1");

        let peer = LoroDoc::new();
        peer.import(&state.snapshot().expect("snapshot"))
            .expect("import");
        let Some(ValueOrContainer::Container(Container::Map(meta))) =
            peer.get_map("panes").get(&key)
        else {
            panic!("pane meta must be a map");
        };
        let Some(ValueOrContainer::Container(Container::Text(body))) = meta.get("body") else {
            panic!("body must be a text container");
        };
        body.update(
            "clobbered",
            loro::UpdateOptions {
                timeout_ms: None,
                use_refined_diff: true,
            },
        )
        .expect("remote edit");
        peer.commit();

        let Imported { merge, .. } = state
            .import(&peer.export(ExportMode::Snapshot).expect("export"))
            .expect("merge");
        assert_eq!(merge, Merge::Applied);
        assert_eq!(
            state.panes[&key].text("body"),
            "clobbered",
            "the merge must reach the container"
        );

        // The producer's next tick still reports "hello", and must win.
        let delta = state
            .apply_scope("p", &[terminal("t1", "hello")])
            .expect("re-apply");
        assert!(
            delta.is_some(),
            "producer tick must re-assert its own value over a remote edit"
        );
        assert_eq!(state.panes[&key].text("body"), "hello");
    }

    /// The other direction of the same cache invariant: a resync must restore
    /// the caches exactly, so an import cannot make an idle pane look changed
    /// and start a delta per tick on a document nobody is touching.
    #[test]
    fn an_import_does_not_make_an_idle_pane_look_changed() {
        let mut state = DocState::new();
        state
            .apply_scope("p", &[terminal("t1", "hello")])
            .expect("apply");

        let peer = LoroDoc::new();
        peer.import(&state.snapshot().expect("snapshot"))
            .expect("import");
        peer.get_map("inputs")
            .insert("t1.verdict", "splice")
            .expect("write");
        peer.commit();
        state
            .import(&peer.export(ExportMode::Snapshot).expect("export"))
            .expect("merge");

        assert!(
            state
                .apply_scope("p", &[terminal("t1", "hello")])
                .expect("re-apply")
                .is_none(),
            "an unchanged tick after an import must still be a no-op"
        );
    }

    /// An update whose dependencies are absent is recorded but not applied, and
    /// saying so is the whole point of the return type: treated as success, the
    /// human's edit is silently invisible.
    #[test]
    fn an_update_missing_its_dependencies_reports_pending() {
        let peer = LoroDoc::new();
        peer.get_map("inputs").insert("first", "a").expect("write");
        peer.commit();
        let after_first = peer.oplog_vv();
        peer.get_map("inputs").insert("second", "b").expect("write");
        peer.commit();
        let tail_only = peer
            .export(ExportMode::updates(&after_first))
            .expect("export");

        let hub = Hub::new();
        assert_eq!(
            hub.import(&tail_only).expect("import"),
            Merge::Pending,
            "an update whose deps are absent is recorded, not applied"
        );

        let reader = LoroDoc::new();
        reader
            .import(&hub.export_snapshot())
            .expect("reader imports");
        let dump = format!("{:?}", reader.get_deep_value());
        assert!(
            !dump.contains("second"),
            "a pending op must not be visible in the document: {dump}"
        );
    }

    /// The core multi-producer invariant: one producer's reconcile never touches
    /// another's panes, and dropping a producer keeps the final snapshot.
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

        // Disconnecting producer a keeps the last visible state.
        state.remove_scope("a").unwrap();
        assert_eq!(state.panes.len(), 2);
        assert!(state.panes.keys().any(|key| key.starts_with("a\u{1f}")));
        assert!(state.panes.keys().any(|key| key.starts_with("b\u{1f}")));
    }

    /// A tick that changes nothing produces no delta, so idle producers do not
    /// spam every connected browser.
    #[test]
    fn unchanged_apply_yields_no_delta() {
        let mut state = DocState::new();
        assert!(
            state
                .apply_scope("a", &[terminal("1", "x")])
                .unwrap()
                .is_some()
        );
        assert!(
            state
                .apply_scope("a", &[terminal("1", "x")])
                .unwrap()
                .is_none()
        );
        // A screen change does produce a delta.
        assert!(
            state
                .apply_scope("a", &[terminal("1", "y")])
                .unwrap()
                .is_some()
        );
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

    #[test]
    fn removing_scope_retains_last_panes() {
        let mut state = DocState::new();
        state.apply_scope("a", &[terminal("1", "last")]).unwrap();
        let key = doc_key("a", "1");
        assert!(state.panes.contains_key(&key));
        assert!(state.remove_scope("a").unwrap().is_none());
        assert_eq!(state.panes[&key].text("body"), "last");
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
                    duration_ms: Some(9),
                    topic: Some("test".to_owned()),
                    line: None,
                    error_line: None,
                    trace: Vec::new(),
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
                duration_ms: None,
                topic: None,
                line: None,
                error_line: None,
                trace: Vec::new(),
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
                duration_ms: Some(4),
                topic: Some("test".to_owned()),
                line: None,
                error_line: None,
                trace: vec![ExecTraceLine {
                    line: 1,
                    text: "hi\n".to_owned(),
                }],
            },
        );
        let finished = std::slice::from_ref(&finished);
        assert!(state.apply_scope("p", finished).unwrap().is_some());
        assert_eq!(state.panes[&key].text("stdout"), "hi\n");
        assert!(
            state.panes[&key].meta.get("ok").is_some(),
            "ok present when done"
        );
        // The inline-trace map round-trips through the doc as JSON text, so the
        // frontend can parse it back (it is dropped if the projection omits it).
        let trace: Vec<ExecTraceLine> =
            serde_json::from_str(&state.panes[&key].text("trace")).expect("trace round-trips");
        assert_eq!(
            trace,
            vec![ExecTraceLine {
                line: 1,
                text: "hi\n".to_owned()
            }]
        );

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

    /// A browser: a second document started from the hub's snapshot, which is
    /// exactly what `/events` hands a real one.
    fn viewer(hub: &Hub) -> LoroDoc {
        let doc = LoroDoc::new();
        doc.import(&hub.export_snapshot())
            .expect("snapshot imports");
        doc
    }

    /// What a viewer posts to `/apply`.
    fn posted(doc: &LoroDoc) -> Vec<u8> {
        doc.export(ExportMode::Snapshot).expect("export")
    }

    /// The note container as a viewer reaches it. Panics rather than creating
    /// one: a viewer that has to create the container is the bug
    /// [`Hub::declare_note`] exists to prevent.
    fn viewer_note(doc: &LoroDoc, scope: &str, pane: &str, field: &str) -> LoroText {
        let Some(ValueOrContainer::Container(Container::Text(text))) =
            doc.get_map(INPUTS_ROOT).get(&input_key(scope, pane, field))
        else {
            panic!("the declared note must be in the snapshot the viewer imported");
        };
        text
    }

    /// The hub's peer introduces itself inside the very change it first writes,
    /// so no change in the history is ever unattributed.
    #[test]
    fn the_first_change_carries_its_own_attribution() {
        let hub = Hub::new();
        hub.apply_scope("p", &[terminal("t1", "hello")]);

        let history = hub.history();
        assert_eq!(history.len(), 1, "one apply, one change");
        assert_eq!(history[0].actor, Some(Actor::agent("producer")));
        assert!(
            history[0]
                .ops
                .iter()
                .any(|op| matches!(op, HistoryOp::MapSet { key, .. } if key == "label")),
            "the peer entry must ride in the pane change, not trail it: {:?}",
            history[0].ops
        );

        // The shape a browser reads out of `doc.toJSON()`. Asserted through a
        // second document because that is where the frontend stands.
        let reader = LoroDoc::new();
        reader
            .import(&hub.export_snapshot())
            .expect("reader imports");
        let peers: serde_json::Value =
            serde_json::to_value(reader.get_map(PEERS_ROOT).get_deep_value())
                .expect("peers serialise");
        let peer = hub.actors().into_keys().next().expect("one peer");
        assert_eq!(
            peers[peer.to_string()],
            serde_json::json!({ "kind": "agent", "label": "producer" })
        );
    }

    /// An owner that names itself late still owns its earlier changes: the entry
    /// is one map key resolved when the history is read, not a label copied into
    /// each change as it is written.
    #[test]
    fn a_late_identity_relabels_earlier_changes() {
        let hub = Hub::new();
        hub.apply_scope("p", &[terminal("t1", "hello")]);
        assert_eq!(hub.history()[0].actor, Some(Actor::agent("producer")));

        hub.declare_identity(&Actor::human("ada"));
        assert_eq!(
            hub.actors().into_values().collect::<Vec<_>>(),
            vec![Actor::human("ada")]
        );
        assert_eq!(hub.history()[0].actor, Some(Actor::human("ada")));
    }

    /// Loro merges consecutive local commits unless something about them differs
    /// -- its merge interval is 1000 *seconds*, so wall-clock spacing will not do
    /// it -- and a history UI that shows a whole session as one change is no
    /// history at all. Each apply has to stay its own change.
    #[test]
    fn each_apply_is_its_own_change() {
        let mut state = DocState::new();
        for tick in 0..5 {
            state
                .apply_scope("p", &[terminal("t1", &format!("screen {tick}"))])
                .expect("apply");
        }
        assert_eq!(state.doc.len_changes(), 5, "five applies, five changes");

        let messages: Vec<String> = state
            .history()
            .into_iter()
            .filter_map(|change| change.message)
            .collect();
        assert_eq!(messages.len(), 5);
        assert_eq!(
            messages.iter().collect::<HashSet<_>>().len(),
            5,
            "identical messages are what merges changes: {messages:?}"
        );
        assert_eq!(
            messages[0],
            r#"{"on":"panes","seq":1,"scope":"p","add":["t1"]}"#
        );
        assert_eq!(
            messages[4],
            r#"{"on":"panes","seq":5,"scope":"p","set":["t1"]}"#
        );
    }

    /// Free text merges: two viewers typing into one note both keep what they
    /// wrote. This is the property a map key would silently destroy, and the
    /// reason a note is a text container.
    #[test]
    fn a_note_keeps_both_viewers_edits() {
        let hub = Hub::new();
        hub.apply_scope("p", &[terminal("t1", "hello")]);
        hub.declare_note("p", "t1", "review").expect("declare");

        // Both fork from the same snapshot, so neither has seen the other.
        let one = viewer(&hub);
        viewer_note(&one, "p", "t1", "review")
            .insert(0, "ship it")
            .expect("type");
        one.commit();
        let two = viewer(&hub);
        viewer_note(&two, "p", "t1", "review")
            .insert(0, "hold on ")
            .expect("type");
        two.commit();

        hub.import(&posted(&one)).expect("merge one");
        hub.import(&posted(&two)).expect("merge two");

        let note = hub.note("p", "t1", "review").expect("note present");
        assert_eq!(
            note.len(),
            "ship it".len() + "hold on ".len(),
            "no edit may be dropped: {note}"
        );
        assert!(
            note.contains("ship it") && note.contains("hold on "),
            "{note}"
        );
    }

    /// A single-answer field is last-write-wins on purpose: two viewers answering
    /// produce one answer, and the newest one is it.
    #[test]
    fn a_choice_keeps_one_answer() {
        let hub = Hub::new();
        hub.apply_scope("p", &[terminal("t1", "hello")]);
        let key = input_key("p", "t1", "verdict");

        let first = viewer(&hub);
        first
            .get_map(INPUTS_ROOT)
            .insert(&key, "hold")
            .expect("answer");
        first.commit();
        hub.import(&posted(&first)).expect("merge");
        assert_eq!(hub.choice("p", "t1", "verdict").as_deref(), Some("hold"));

        // A viewer that has seen the first answer overwrites it.
        let second = viewer(&hub);
        second
            .get_map(INPUTS_ROOT)
            .insert(&key, "ship")
            .expect("answer");
        second.commit();
        hub.import(&posted(&second)).expect("merge");
        assert_eq!(hub.choice("p", "t1", "verdict").as_deref(), Some("ship"));
        assert_eq!(hub.inputs().len(), 1, "one field holds one answer");
    }

    /// The read side a producer polls: both shapes come back parsed, and a
    /// checkbox's `true` reads as an answer rather than as nothing.
    #[test]
    fn inputs_read_back_with_their_merge_shape() {
        let hub = Hub::new();
        hub.apply_scope("p", &[terminal("t1", "hello")]);
        hub.declare_note("p", "t1", "review").expect("declare");

        let browser = viewer(&hub);
        browser
            .get_map(INPUTS_ROOT)
            .insert(&input_key("p", "t1", "approve"), true)
            .expect("tick the box");
        viewer_note(&browser, "p", "t1", "review")
            .insert(0, "looks good")
            .expect("type");
        browser.commit();
        hub.import(&posted(&browser)).expect("merge");

        assert_eq!(
            hub.inputs(),
            vec![
                InputEntry {
                    scope: "p".to_owned(),
                    pane: "t1".to_owned(),
                    field: "approve".to_owned(),
                    value: Input::Choice {
                        value: "true".to_owned()
                    },
                },
                InputEntry {
                    scope: "p".to_owned(),
                    pane: "t1".to_owned(),
                    field: "review".to_owned(),
                    value: Input::Note {
                        text: "looks good".to_owned()
                    },
                },
            ]
        );
        assert_eq!(hub.choice("p", "t1", "approve").as_deref(), Some("true"));
        assert_eq!(hub.note("p", "t1", "review").as_deref(), Some("looks good"));
        // A note read as a choice is not an answer, and neither is the reverse:
        // getting those two crossed is what loses a colleague's sentence.
        assert_eq!(hub.choice("p", "t1", "review"), None);
        assert_eq!(hub.note("p", "t1", "approve"), None);
    }

    /// The ownership partition, both directions: a producer tick rewrites its
    /// pane and leaves the answers alone, and an answer leaves the pane alone.
    #[test]
    fn a_producer_tick_and_a_viewer_answer_do_not_collide() {
        let mut state = DocState::new();
        state
            .apply_scope("p", &[terminal("t1", "before")])
            .expect("apply");
        assert!(
            state
                .declare_note("p", "t1", "review")
                .expect("declare")
                .is_some(),
            "a declared note has to reach viewers that are already connected"
        );

        let browser = LoroDoc::new();
        browser
            .import(&state.snapshot().expect("snapshot"))
            .expect("import");
        browser
            .get_map(INPUTS_ROOT)
            .insert(&input_key("p", "t1", "verdict"), "hold")
            .expect("answer");
        let Some(ValueOrContainer::Container(Container::Text(note))) = browser
            .get_map(INPUTS_ROOT)
            .get(&input_key("p", "t1", "review"))
        else {
            panic!("declared note must be in the snapshot");
        };
        note.insert(0, "typed").expect("type");
        browser.commit();
        state
            .import(&browser.export(ExportMode::Snapshot).expect("export"))
            .expect("merge");

        let key = doc_key("p", "t1");
        assert_eq!(
            state.panes[&key].text("body"),
            "before",
            "an answer is not a pane edit"
        );

        state
            .apply_scope("p", &[terminal("t1", "after")])
            .expect("apply");
        assert_eq!(state.panes[&key].text("body"), "after");
        assert_eq!(state.choice("p", "t1", "verdict").as_deref(), Some("hold"));
        assert_eq!(state.note("p", "t1", "review").as_deref(), Some("typed"));
    }

    /// What a history UI renders for a viewer's edit: the viewer's own name, the
    /// change it built on, and the size of the text it typed. The position is
    /// deliberately absent -- Loro's JSON `pos` is an entity index, not an offset
    /// into the body.
    #[test]
    fn history_attributes_a_viewer_edit_to_the_viewer() {
        let hub = Hub::new();
        hub.apply_scope("p", &[terminal("t1", "hello")]);
        hub.declare_note("p", "t1", "review").expect("declare");

        let browser = viewer(&hub);
        // A browser introduces itself into `__peers` exactly as the hub does.
        write_actor(
            &browser.get_map(PEERS_ROOT),
            browser.peer_id(),
            &Actor::human("ada"),
        )
        .expect("introduce");
        viewer_note(&browser, "p", "t1", "review")
            .insert(0, "looks good")
            .expect("type");
        browser.commit();
        hub.import(&posted(&browser)).expect("merge");

        let change = hub
            .history()
            .into_iter()
            .find(|change| change.peer == browser.peer_id().to_string())
            .expect("the viewer's change is in the history");
        assert_eq!(change.actor, Some(Actor::human("ada")));
        assert_eq!(
            change.deps.len(),
            1,
            "it builds on the snapshot it imported"
        );
        assert!(
            change
                .ops
                .iter()
                .any(|op| matches!(op, HistoryOp::TextInsert { chars, .. } if *chars == 10)),
            "the typed run must be reported by length: {:?}",
            change.ops
        );
        assert!(
            hub.history().iter().all(|change| change.actor.is_some()),
            "every change in the document is attributable"
        );
    }
}
