//! Sample a live [`TuiManager`](crate::TuiManager) into the engine-free wire
//! panes shared by the producer ([`crate::publish`]) and the in-process
//! dashboard ([`crate::dashboard`]).
//!
//! The wire shapes themselves ([`Pane`], [`TerminalView`], [`ProducerSnapshot`])
//! and the discovery paths live in `dashboard-core` so the aggregator can render
//! them without the PTY engine; `tui` re-exports them. This module owns only the
//! bridge that reads a terminal out of a manager and wraps it as a pane, which is
//! the one half that needs the engine.
//!
//! Sampling is stateful: a [`PaneCollector`] carries per-terminal activity
//! (for the working/awaiting/gate/completed status, [`status`]) and each
//! agent's transcript tail ([`crate::transcript`]) across ticks, publishing a
//! `DataView` transcript pane beside every agent terminal. State for panes
//! the manager no longer tracks is pruned each tick.
//!
//! The collector is also where a browser learns whether its message actually
//! reached a terminal: the producer's delivery task records each send's
//! outcome in a shared [`SendLog`] and the collector publishes it as a second
//! `DataView` pane under the terminal. Without it a swallowed send is
//! indistinguishable from a delivered one from the browser's side, which is
//! the state ENG-12530 found.

pub mod status;

mod sgr;

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use dashboard_core::{Pane, TerminalView};
use parking_lot::Mutex;
use serde::Serialize;
use uuid::Uuid;

use crate::transcript::Tail;
use crate::types::AgentConfig;

/// How many send outcomes a terminal keeps. A person reading the card wants
/// the last few; the browser only needs the one whose id it is waiting on,
/// and a bounded list keeps a long-lived pane from growing without end.
const KEEP_SENDS: usize = 20;

/// What became of one browser-submitted message.
///
/// Published so the browser can tell a send that *landed* from one the
/// aggregator merely accepted. Those are different outcomes and used to look
/// identical: a terminal sitting on an onboarding gate swallows keystrokes
/// whole, and the submission returned quietly with nothing on the pane, in
/// the scrollback or in any log to say the message was gone (ENG-12530).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SendOutcome {
    /// The uuid the browser stamped on the send, so it can match this record
    /// to the message it submitted.
    pub id: String,
    /// `"landed"` when the terminal echoed the text back, `"unconfirmed"`
    /// when nothing came back to prove it arrived.
    pub state: &'static str,
    /// The opening of the message, so a person reading the card knows which
    /// one this is.
    pub preview: String,
    /// For an unconfirmed send, what the producer saw instead -- including
    /// the on-screen prompt that is swallowing the keystrokes, when there is
    /// one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// Where send outcomes are recorded, shared between the two halves of a
/// producer.
///
/// The delivery task and the poll loop are separate tasks over one manager,
/// so a return value cannot get an outcome from the first to the second; a
/// cloneable handle can. Cheap to clone and cheap to read: the poll loop asks
/// once per terminal per tick and almost always gets nothing.
#[derive(Clone, Default)]
pub struct SendLog {
    outcomes: Arc<Mutex<HashMap<Uuid, Vec<SendOutcome>>>>,
}

impl SendLog {
    /// An empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record what became of one send to `pane`, dropping the oldest record
    /// once the window is full.
    pub fn record(&self, pane: Uuid, outcome: SendOutcome) {
        let mut outcomes = self.outcomes.lock();
        let pane_outcomes = outcomes.entry(pane).or_default();
        pane_outcomes.push(outcome);
        if pane_outcomes.len() > KEEP_SENDS {
            let excess = pane_outcomes.len() - KEEP_SENDS;
            pane_outcomes.drain(..excess);
        }
    }

    /// This pane's outcomes, oldest first.
    #[must_use]
    pub fn outcomes(&self, pane: Uuid) -> Vec<SendOutcome> {
        self.outcomes.lock().get(&pane).cloned().unwrap_or_default()
    }

    /// Forget panes the manager no longer tracks.
    fn retain(&self, live: &HashSet<Uuid>) {
        self.outcomes.lock().retain(|id, _| live.contains(id));
    }
}

/// Stateful pane sampler: one per poll loop.
pub struct PaneCollector {
    activity: HashMap<Uuid, status::Activity>,
    transcripts: HashMap<Uuid, Tail>,
    sends: SendLog,
}

impl Default for PaneCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl PaneCollector {
    #[must_use]
    pub fn new() -> Self {
        Self::with_sends(SendLog::new())
    }

    /// A collector that publishes the outcomes recorded in `sends` beside the
    /// terminals they belong to. [`new`](Self::new) is this with a log nobody
    /// writes to, for a caller that does not deliver browser input.
    #[must_use]
    pub fn with_sends(sends: SendLog) -> Self {
        Self {
            activity: HashMap::new(),
            transcripts: HashMap::new(),
            sends,
        }
    }

    /// Sample every terminal the manager tracks into a pane list: one
    /// terminal pane each, plus one transcript pane per agent whose session
    /// log is being tailed.
    ///
    /// A terminal whose styled-cell read fails this tick is skipped, not
    /// dropped from the set: the next tick re-reads it. The screen is encoded
    /// as minimal ANSI SGR ([`sgr::encode`]) so the dashboard can paint color
    /// and attributes; the cursor position and shape and the exit code ride
    /// alongside.
    pub async fn collect(&mut self, manager: &crate::TuiManager) -> Vec<Pane> {
        let mut panes = Vec::new();
        let mut live: HashSet<Uuid> = HashSet::new();
        for instance in manager.list() {
            let Ok(cells) = instance.read_styled_cells_async().await else {
                continue;
            };
            live.insert(instance.id);
            let viewport = instance.read_viewport_async().await.unwrap_or_default();
            let scrollback = instance
                .read_scrollback_async()
                .await
                .map(|lines| lines.join("\n"))
                .unwrap_or_default();
            // Cursor and exit are best-effort: a failed cursor read defaults to the
            // top-left, never dropping the whole pane.
            let cursor = instance
                .read_cursor_async()
                .await
                .unwrap_or(crate::CursorPos {
                    row: 0,
                    col: 0,
                    visible: true,
                });
            let rows: Vec<Vec<crate::StyledCell>> =
                cells.rows().into_iter().map(|row| row.to_vec()).collect();

            let agent = instance.agent.as_deref();
            let exit_state = instance.exit_state();
            let activity = self.activity.entry(instance.id).or_insert_with(status::Activity::new);
            activity.observe(
                &viewport,
                agent.and_then(|config| config.busy_marker.as_deref()),
                agent.map_or(&[], |config| config.gate_markers.as_slice()),
            );
            let current_status = activity.status(exit_state);

            panes.push(Pane::terminal(
                instance.id.to_string(),
                TerminalView {
                    command: instance.command.clone(),
                    args: instance.args.join(" "),
                    rows: instance.rows(),
                    cols: instance.cols(),
                    alive: instance.is_alive(),
                    screen: sgr::encode(&rows),
                    scrollback,
                    cursor_row: cursor.row,
                    cursor_col: cursor.col,
                    cursor_visible: cursor.visible,
                    cursor_shape: instance.cursor_shape().as_str().to_owned(),
                    exit_code: match exit_state {
                        crate::ExitState::Exited(code) => code,
                        crate::ExitState::Running => None,
                    },
                    status: Some(current_status.as_str().to_owned()),
                    agent: agent.map(|config| config.kind.clone()),
                },
            ));

            if let Some(pane) = self.transcript_pane(&instance, agent) {
                panes.push(pane);
            }
            if let Some(pane) = self.sends_pane(&instance) {
                panes.push(pane);
            }
        }
        // Panes the manager dropped take their per-tick state with them, so a
        // long-lived producer does not accumulate dead activity entries.
        self.activity.retain(|id, _| live.contains(id));
        self.transcripts.retain(|id, _| live.contains(id));
        self.sends.retain(&live);
        panes
    }

    /// The send-outcome pane for one terminal, once a browser has submitted
    /// something to it.
    ///
    /// A pane rather than a field on the terminal view, for two reasons. The
    /// aggregator projects a terminal view into a fixed set of scalars and
    /// two text containers, so a list would have to be smuggled through as
    /// JSON text anyway; and a `data` pane with an unregistered renderer name
    /// already renders in the browser as a JSON tree, so the failure this
    /// exists to expose is visible today rather than after a frontend change
    /// -- which was exactly the complaint (ENG-12530). It nests under the
    /// terminal, like the transcript pane, and does not exist at all until
    /// there is something to say.
    fn sends_pane(&self, instance: &crate::TuiInstance) -> Option<Pane> {
        let outcomes = self.sends.outcomes(instance.id);
        if outcomes.is_empty() {
            return None;
        }
        let mut pane = Pane::data(
            format!("{}-sends", instance.id),
            "browser sends",
            "sends",
            serde_json::json!({ "sends": outcomes }),
        );
        pane.parent = Some(instance.id.to_string());
        Some(pane)
    }

    /// The transcript pane for one agent terminal, when its config names a
    /// session-log family. Rows only append (within the tail's window), so
    /// the pane body diffs incrementally in the hub's Loro text container.
    fn transcript_pane(
        &mut self,
        instance: &crate::TuiInstance,
        agent: Option<&AgentConfig>,
    ) -> Option<Pane> {
        let config = agent?;
        let log_kind = config.session_log?;
        let tail = self.transcripts.entry(instance.id).or_insert_with(|| {
            Tail::new(
                log_kind,
                config.cwd.clone(),
                config.log_root.clone(),
                instance.spawned_at,
            )
        });
        let _ = tail.poll();
        let mut pane = Pane::data(
            format!("{}-transcript", instance.id),
            format!("{} transcript", config.kind),
            "transcript",
            serde_json::json!({
                "agent": config.kind,
                "entries": tail.entries,
                // A climbing count is the format-drift alarm; zero means every
                // line parsed. Never hidden.
                "skipped": tail.skipped,
            }),
        );
        pane.parent = Some(instance.id.to_string());
        Some(pane)
    }
}
