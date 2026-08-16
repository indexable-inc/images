//! Status inference for one terminal: working, awaiting input, gated, or
//! completed, from screen activity alone.
//!
//! A PTY has no "turn finished" event, so this is the same inference the
//! Python harness and fleetview use: **quiescence first, marker second**. The
//! viewport must hold still for [`SETTLE`] and the agent's busy footer (when
//! it has a grounded one) must be absent; the marker is the precise fast path
//! that catches an agent thinking silently longer than the settle window.
//! This is fleetview's `session.rs` inference ported to where the viewport is
//! already sampled per tick; fleetview's copy stays until it consumes
//! `tui::publish` (phase 6 of ENG-12457) and deletes it.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher as _};
use std::time::{Duration, Instant};

use crate::types::ExitState;

/// Blocking-question fragments, when the agent config names none. Grounded
/// against Claude Code's permission prompts (shared with fleetview).
const DEFAULT_GATE_MARKERS: [&str; 4] =
    ["do you want", "would you like", "❯ 1. yes", "1. yes, and"];

/// How long the screen must hold still before a session counts as idle. Long
/// enough to survive a spinner frame or a token trickling in, short enough
/// that the board reacts within a breath of the agent stopping.
const SETTLE: Duration = Duration::from_millis(900);

/// What a session is doing, as the wire spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The screen is changing, or the busy footer is up.
    Working,
    /// Alive and quiet: the turn ended.
    AwaitingInput,
    /// Alive, quiet, and a blocking question is on screen: awaiting input of
    /// the kind that blocks the agent rather than ends the turn.
    Gate,
    /// The process exited.
    Completed,
}

impl Status {
    /// The wire token, stored on the pane and read by the browser.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::AwaitingInput => "awaiting_input",
            Self::Gate => "gate",
            Self::Completed => "completed",
        }
    }
}

/// Rolling activity state for one terminal, fed one viewport read per tick.
pub struct Activity {
    last_change: Instant,
    digest: u64,
    busy: bool,
    gate: bool,
}

impl Activity {
    pub fn new() -> Self {
        Self {
            last_change: Instant::now(),
            digest: 0,
            busy: false,
            gate: false,
        }
    }

    /// Fold one viewport read into the state. `busy_marker` is the agent's
    /// grounded in-flight footer, `gate_markers` its blocking-question
    /// fragments (both case-insensitive; empty gate set uses the defaults).
    pub fn observe(&mut self, lines: &[String], busy_marker: Option<&str>, gate_markers: &[String]) {
        let digest = digest(lines);
        if digest != self.digest {
            self.digest = digest;
            self.last_change = Instant::now();
        }
        let haystack = lines.join("\n").to_lowercase();
        self.busy = busy_marker
            .is_some_and(|marker| haystack.contains(&marker.to_lowercase()));
        self.gate = gate_line(lines, gate_markers).is_some();
    }

    /// The status right now. An exited process is completed whatever the
    /// screen says; a running one whose screen is still moving, or whose
    /// footer says it is thinking, is working; anything else wants a human,
    /// and a blocking question upgrades that to a gate.
    #[must_use]
    pub fn status(&self, exit: ExitState) -> Status {
        match exit {
            ExitState::Exited(_) => Status::Completed,
            ExitState::Running if self.busy || self.last_change.elapsed() < SETTLE => {
                Status::Working
            }
            ExitState::Running if self.gate => Status::Gate,
            ExitState::Running => Status::AwaitingInput,
        }
    }

    /// Test hook: pretend the screen has been still since `ago`.
    #[cfg(test)]
    pub fn settle_for_test(&mut self, ago: Duration) {
        self.last_change = Instant::now()
            .checked_sub(ago)
            .expect("test settle offsets are far inside the Instant range");
    }
}

/// The blocking question a gated session is sitting on, as it reads on
/// screen.
///
/// Empty `markers` falls back to [`DEFAULT_GATE_MARKERS`], the same rule
/// [`Activity::observe`] applies. Matching is per line rather than over the
/// joined screen so the hit can be *reported* and not merely counted -- the
/// producer quotes it when browser keystrokes go nowhere, which is
/// the difference between "your message vanished" and "the agent is waiting
/// on this prompt". Every grounded marker is a fragment of one line, so
/// nothing is lost by not searching across the join.
#[must_use]
pub fn gate_line(lines: &[String], markers: &[String]) -> Option<String> {
    lines
        .iter()
        .find(|line| {
            let lowered = line.to_lowercase();
            if markers.is_empty() {
                DEFAULT_GATE_MARKERS
                    .iter()
                    .any(|marker| lowered.contains(marker))
            } else {
                markers
                    .iter()
                    .any(|marker| lowered.contains(&marker.to_lowercase()))
            }
        })
        .map(|line| line.trim().to_owned())
}

/// A stable digest of the rendered screen. Any cell that changed changes it.
fn digest(lines: &[String]) -> u64 {
    let mut hasher = DefaultHasher::new();
    lines.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    const BUSY: Option<&str> = Some("esc to interrupt");

    /// A quiet screen with a blocking question is a gate, not plain awaiting.
    #[test]
    fn a_question_on_a_settled_screen_is_a_gate() {
        let mut activity = Activity::new();
        activity.observe(&lines("Do you want to run this command?\n ❯ 1. Yes"), BUSY, &[]);
        activity.settle_for_test(Duration::from_secs(5));
        assert_eq!(activity.status(ExitState::Running), Status::Gate);
    }

    /// The busy footer wins over a question fragment: a working session is
    /// never a gate.
    #[test]
    fn a_working_session_is_never_a_gate() {
        let mut activity = Activity::new();
        activity.observe(
            &lines("Do you want to proceed? (esc to interrupt)"),
            BUSY,
            &[],
        );
        activity.settle_for_test(Duration::from_secs(5));
        assert_eq!(activity.status(ExitState::Running), Status::Working);
    }

    /// An exited process is completed whatever the screen says.
    #[test]
    fn an_exited_process_is_completed_whatever_the_screen_says() {
        let mut activity = Activity::new();
        activity.observe(&lines("still painting (esc to interrupt)"), BUSY, &[]);
        assert_eq!(
            activity.status(ExitState::Exited(Some(2))),
            Status::Completed
        );
    }

    /// A marker-less agent falls back to quiescence: moving screen works,
    /// settled screen awaits.
    #[test]
    fn quiescence_is_the_marker_less_backstop() {
        let mut activity = Activity::new();
        activity.observe(&lines("thinking hard"), None, &[]);
        assert_eq!(
            activity.status(ExitState::Running),
            Status::Working,
            "a fresh change means working"
        );
        activity.settle_for_test(Duration::from_secs(5));
        assert_eq!(activity.status(ExitState::Running), Status::AwaitingInput);
    }

    /// Config-supplied gate markers replace the defaults.
    #[test]
    fn config_gate_markers_replace_the_defaults() {
        let mut activity = Activity::new();
        let markers = vec!["approve this plan".to_owned()];
        activity.observe(&lines("Approve this plan?"), None, &markers);
        activity.settle_for_test(Duration::from_secs(5));
        assert_eq!(activity.status(ExitState::Running), Status::Gate);

        activity.observe(&lines("Do you want to run this command?"), None, &markers);
        activity.settle_for_test(Duration::from_secs(5));
        assert_eq!(
            activity.status(ExitState::Running),
            Status::AwaitingInput,
            "the default fragments must not fire once the config names its own"
        );
    }
}
