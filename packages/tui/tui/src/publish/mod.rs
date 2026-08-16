//! Producer side: expose this process's terminals as panes over a unix socket so
//! the out-of-process aggregator can render them.
//!
//! The socket, the wire serialization, and the fan-out all live in
//! [`dashboard_core::Publisher`]; this module is the one adapter that needs the
//! PTY engine. [`publish`] binds that publisher, then spawns a poll loop on the
//! manager's runtime that samples the manager into panes (a stateful
//! [`PaneCollector`](crate::frame::PaneCollector): terminals plus their
//! status and per-agent transcript panes) and pushes them through the
//! publisher's sink each tick. The poll task is attached to the publisher, so
//! stopping or dropping it winds the loop down with the socket.
//!
//! The socket's return direction makes a browser's message reach the PTY: a
//! second task consumes [`Publisher::inputs`] and, for each `send` input on a
//! pane that maps to one of this manager's terminals, types the text and
//! submits it with the same discipline the Python harness proved out
//! ([`submit`]). Landing this at the PTY layer is the point: every producer
//! built on this module -- Python, Node, Elixir, fleetview -- inherits typed
//! browser input with no per-CLI protocol work.
//!
//! Delivery reports back. Typing at a terminal is not the same as being heard
//! by it -- an agent parked on an onboarding gate ("Enter to confirm")
//! discards every keystroke -- so each send's outcome is recorded in the
//! [`SendLog`] the pane collector publishes. The browser can then tell a send
//! that landed from one that merely left, which it could not before: the
//! submission returned quietly and the message was simply gone, absent from
//! the screen, the scrollback and every log (ENG-12530).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use dashboard_core::Publisher;
use dashboard_core::{Input, InputLine};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::frame::{PaneCollector, SendLog, SendOutcome, status};
use crate::{Error, Result, TuiInstance, TuiManager};

/// Synchronous twin of [`publish`] for callers without a runtime.
///
/// Fleetview's ratatui main is the motivating caller: it blocks on the
/// manager's runtime, which outlives the caller's stack frame by
/// construction.
///
/// # Errors
///
/// Same as [`publish`].
pub fn publish_blocking(
    manager: &Arc<TuiManager>,
    path: PathBuf,
    poll: Duration,
) -> Result<Publisher> {
    manager.runtime_handle().block_on(publish(manager, path, poll))
}

/// Publish `manager`'s terminals as panes on the unix socket at `path`.
///
/// `path` is usually [`socket_path`](crate::socket_path). `poll` is the sampling
/// interval; every tick pushes the current terminal panes to all connected
/// readers. The poll and accept loops run on the manager's runtime, so the
/// producer survives a temporary caller runtime being dropped.
///
/// # Errors
///
/// Returns [`Error::Publish`] when the discovery directory cannot be created or
/// the socket cannot be bound.
///
/// # Panics
///
/// Panics if the freshly bound publisher's inputs receiver has already been
/// taken, which cannot happen: this function is the only caller on that
/// publisher.
pub async fn publish(
    manager: &Arc<TuiManager>,
    path: PathBuf,
    poll: Duration,
) -> Result<Publisher> {
    let runtime = manager.runtime_handle();
    let mut publisher = Publisher::bind(path, &runtime).map_err(|source| Error::Publish {
        message: source.to_string(),
    })?;

    // The collector is stateful (status inference and transcript tails live
    // across ticks), so one instance seeds the first snapshot and then moves
    // into the poll task. The send log is the one piece of state the delivery
    // task writes and the poll task reads, so both hold a handle to it.
    let sends = SendLog::new();
    let mut collector = PaneCollector::with_sends(sends.clone());
    // Seed the first snapshot before returning so a reader that connects
    // immediately sees the current terminals without waiting a poll interval.
    publisher.publish(&collector.collect(manager).await);

    let sink = publisher.sink();
    let poll_manager = manager.clone();
    let poller = runtime.spawn(async move {
        loop {
            tokio::time::sleep(poll).await;
            sink.publish(&collector.collect(&poll_manager).await);
        }
    });
    publisher.push_task(poller);

    // The return direction: viewer inputs routed back by the aggregator.
    // A fresh publisher's receiver is untaken by construction, and this module
    // is the only caller on this publisher.
    let inputs = publisher
        .inputs()
        .expect("a fresh Publisher's inputs receiver is untaken");
    let deliverer = runtime.spawn(deliver_sends(manager.clone(), inputs, sends));
    publisher.push_task(deliverer);

    Ok(publisher)
}

/// The inputs field a browser writes to submit a message to a terminal: an
/// LWW choice holding `{"id": "<uuid>", "text": "..."}` JSON. The shared
/// `compose` draft is a different field -- a draft being typed, never
/// submitted by the producer.
const SEND_FIELD: &str = "send";

/// One submitted message: the id that makes replay idempotent, and the text.
/// Not named `Send`: that would shadow the marker trait for this module.
struct SendValue {
    id: String,
    text: String,
}

/// Consume routed viewer inputs and deliver each new `send` to its terminal.
///
/// Duplicates are normal, not exceptional: the aggregator replays a scope's
/// inputs on every (re)connect, and a value can arrive once live and again
/// replayed. The browser stamps each send with a fresh uuid, so "new" is "an
/// id this pane has not delivered yet"; the map is keyed per pane because the
/// same document holds one `send` field per pane. Recording happens only
/// after the terminal lookup succeeds, so a send racing its pane's spawn is
/// retried by a later replay instead of being marked delivered and lost.
async fn deliver_sends(
    manager: Arc<TuiManager>,
    mut inputs: mpsc::Receiver<InputLine>,
    sends: SendLog,
) {
    let mut delivered: HashMap<String, String> = HashMap::new();
    while let Some(line) = inputs.recv().await {
        if line.field != SEND_FIELD {
            continue;
        }
        // A note under `send` is a draft-shaped mistake, not a submission.
        let Input::Choice { value } = line.value else {
            continue;
        };
        // The value is browser-authored: an unparseable one is skipped the
        // same way the hub skips an invented input key, not an error.
        let Some(send) = parse_send(&value) else {
            continue;
        };
        let Ok(id) = Uuid::parse_str(&line.pane) else {
            continue;
        };
        let Ok(instance) = manager.get(&id) else {
            continue;
        };
        if delivered.get(&line.pane).is_some_and(|last| *last == send.id) {
            continue;
        }
        delivered.insert(line.pane, send.id.clone());
        let delivery = submit(&instance, &send.text).await;
        sends.record(id, delivery.into_outcome(send));
    }
}

/// How far one submitted message got.
///
/// The distinction the browser needs. Everything but [`Landed`](Self::Landed)
/// means the message may never have been read by the program behind the PTY,
/// and saying nothing about that is what ENG-12530 was.
enum Delivery {
    /// The terminal echoed the text back and Enter was pressed: the message
    /// is in the program's input.
    Landed,
    /// The PTY is gone. Nothing was typed, and the pane's exit state already
    /// says why.
    Dead,
    /// The text never appeared on screen within [`ECHO_WAIT`]. Enter was
    /// pressed anyway, as it always has been, but nothing confirms the
    /// message arrived. `gate` is the blocking prompt that is eating the
    /// keystrokes, when the screen shows one.
    NoEcho { gate: Option<String> },
}

impl Delivery {
    /// Turn this into the record the pane publishes.
    fn into_outcome(self, send: SendValue) -> SendOutcome {
        let preview = submit_probe(&send.text);
        let (state, detail) = match self {
            Self::Landed => ("landed", String::new()),
            Self::Dead => (
                "unconfirmed",
                "the terminal's process is gone; nothing was typed".to_owned(),
            ),
            Self::NoEcho { gate: Some(prompt) } => (
                "unconfirmed",
                format!(
                    "the text never appeared on screen; the session is waiting on \
                     a prompt that discards typing -- {prompt}"
                ),
            ),
            Self::NoEcho { gate: None } => (
                "unconfirmed",
                format!(
                    "the text never appeared on screen within {}s of being typed",
                    ECHO_WAIT.as_secs()
                ),
            ),
        };
        SendOutcome {
            id: send.id,
            state,
            preview,
            detail,
        }
    }
}

/// Parse a send value's `{"id": ..., "text": ...}` JSON.
fn parse_send(value: &str) -> Option<SendValue> {
    let parsed: serde_json::Value = serde_json::from_str(value).ok()?;
    Some(SendValue {
        id: parsed.get("id")?.as_str()?.to_owned(),
        text: parsed.get("text")?.as_str()?.to_owned(),
    })
}

/// How long the typed text has to show up on screen before the send counts
/// as unconfirmed. Generous: an agent TUI can be mid-render when the text
/// arrives, and a false "unconfirmed" is a lie in the other direction.
const ECHO_WAIT: Duration = Duration::from_secs(5);

/// Type `text` into `instance`, submit it, and report how far it got: the
/// Rust port of the Python harness's `prompt()`
/// (packages/tui/tui-py/python/tui/harness.py).
///
/// Submitting an agent TUI is racier than a shell: a bare `text + Enter`
/// right after the previous turn can land mid-render, leaving the text typed
/// but unsubmitted. So this types first, waits for the box to echo a prefix
/// of the text, then presses Enter, and presses Enter once more if the turn
/// did not start -- grounded against Claude Code, which drops the occasional
/// fast Enter.
///
/// The echo wait is also the delivery check. It always existed; what it
/// produced was thrown away, so a terminal that ignores keystrokes entirely
/// (an onboarding gate, a program reading raw with echo off) took the send
/// and lost it with nothing to show. Enter is still pressed either way --
/// unchanged, and deliberately so, since deciding *for* a person which
/// prompt to answer is a separate call -- but the caller now learns that
/// nothing confirmed the text, and what was on screen instead.
///
/// "The turn started" is the marker-less arm of harness.py's
/// `_turn_started`: the screen changed. No busy marker is known here, since
/// any command can sit behind a pane. One deliberate difference: the
/// pre-Enter frame is hashed *before* Enter is written, where the Python
/// hashes after -- hashing after can capture a frame that already contains
/// the turn's start and then wait for a second change that never comes,
/// which costs a spurious extra Enter on a quiet program (an agent TUI
/// redraws constantly, a bare `cat` does not).
async fn submit(instance: &TuiInstance, text: &str) -> Delivery {
    if instance.write_async(text).await.is_err() {
        return Delivery::Dead; // nothing to type into; the pane shows why
    }
    // The box may wrap or soft-truncate long input, so probe for a short
    // prefix of the first line only. An empty probe is a bare Enter: there is
    // no text whose arrival could be confirmed, so nothing is claimed.
    let probe = submit_probe(text);
    let echoed = if probe.is_empty() {
        true
    } else {
        wait_for_screen(instance, ECHO_WAIT, |screen| screen.contains(&probe)).await
    };
    let before = screen_text(instance).await;
    if instance.write_async("
").await.is_err() {
        return Delivery::Dead;
    }
    let started = wait_for_screen(instance, Duration::from_secs(2), move |screen| {
        screen != before
    })
    .await;
    if !started {
        let _ = instance.write_async("
").await;
    }
    if echoed {
        Delivery::Landed
    } else {
        Delivery::NoEcho {
            gate: gate_on_screen(instance).await,
        }
    }
}

/// The blocking prompt this terminal is parked on, if any.
///
/// Read after a failed echo to answer the question a bare "it did not arrive"
/// leaves open: whether the message vanished into nothing, or into a question
/// somebody has to answer first. Uses the agent's own gate fragments where
/// the spawner named some and the grounded defaults otherwise -- the same
/// rule the status badge applies.
async fn gate_on_screen(instance: &TuiInstance) -> Option<String> {
    let lines = instance.read_viewport_async().await.ok()?;
    let markers: &[String] = instance
        .agent
        .as_deref()
        .map_or(&[], |config| config.gate_markers.as_slice());
    status::gate_line(&lines, markers)
}

/// A short prefix of `text` to confirm it landed in the input box: the first
/// non-empty line, capped at 24 chars (harness.py `_submit_probe`). Agent
/// boxes wrap or soft-truncate long input, so matching the whole text is
/// unreliable.
fn submit_probe(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(24)
        .collect()
}

/// Poll the viewport until `matches`, or until `timeout`; reports whether it
/// matched. 100ms matches the Python harness's polling cadence.
async fn wait_for_screen(
    instance: &TuiInstance,
    timeout: Duration,
    // `Send` because the future crosses threads on the manager's runtime;
    // the screen is read into a local first so no borrow of `matches` is
    // held across an await.
    matches: impl Fn(&str) -> bool + Send,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let screen = screen_text(instance).await;
        if matches(&screen) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The viewport as one string; a failed read is an empty screen for the
/// purposes of waiting, never a reason to stop the delivery task.
async fn screen_text(instance: &TuiInstance) -> String {
    instance
        .read_viewport_async()
        .await
        .map(|lines| lines.join("\n"))
        .unwrap_or_default()
}
