//! Live activation view for `nwm home switch` / `nwm os switch`.
//!
//! A "switch" is two phases Nix's internal-json stream does not cover: the
//! `nix build` of the toplevel (which the build tree already renders) and then
//! running the generation's `activate` script. The activate script is where the
//! work the operator actually waits on happens (linking files, installing
//! packages, restarting launch agents), and it reports its own progress as plain
//! stdout lines, not internal-json. This module folds those lines into a small,
//! wire-friendly [`Activation`] subtree the UI renders beside the build tree.
//!
//! home-manager emits a fixed, named step sequence via `_iNote "Activating %s"`,
//! so each `Activating <step>` line opens a step and closes the previous one,
//! giving a real step tree. nix-darwin's `activate` is unstructured, so the
//! server seeds a single step up front and every line lands under it.
//!
//! Kept pure and `now_ms`-injected (like [`crate::daemon::DaemonTrace`]) so the
//! classifier is unit-testable without a clock; the server stamps the time and
//! drives it through [`crate::MonitorState`].

use serde::{Deserialize, Serialize};

/// Prefix home-manager prints (via `_iNote "Activating %s"`) before each
/// activation step. The remainder of the line is the step name.
const ACTIVATING_PREFIX: &str = "Activating ";

/// home-manager's banner line at the top of activation. Recognised so a stray
/// leading line does not become an unnamed step; carries no step itself.
const HM_BANNER: &str = "Starting Home Manager activation";

/// Status of one activation step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationStatus {
    Running,
    Done,
    Failed,
}

/// One activation step: a named unit of the activate script's work plus the
/// lines it printed while running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationStep {
    pub name: String,
    pub status: ActivationStatus,
    /// Output lines captured while this step was the open one.
    pub lines: Vec<String>,
    pub started_at_ms: u64,
    pub stopped_at_ms: Option<u64>,
}

/// Wire-friendly snapshot of the activation phase.
///
/// `active` mirrors `DaemonInfo::tracing` (the panel greys out until a switch
/// starts); `status` mirrors `DaemonInfo::status` (a human line explaining the
/// current phase, e.g. "running", "skipped (build failed)", "failed").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activation {
    /// Whether an activation phase has begun. False on a plain `nix build`, so
    /// the UI hides the panel entirely.
    pub active: bool,
    /// The activation command being run (e.g. `<out>/activate`).
    pub command: String,
    pub steps: Vec<ActivationStep>,
    /// Human-readable phase status, always populated once `active`.
    pub status: String,
}

impl Activation {
    /// Begin an activation phase. `initial_step` seeds a first open step for the
    /// unstructured (darwin) case so every line has somewhere to land; pass
    /// `None` for home-manager, whose own `Activating` lines open the steps.
    pub fn begin(&mut self, command: String, initial_step: Option<String>, now_ms: u64) {
        self.active = true;
        self.command = command;
        "running".clone_into(&mut self.status);
        self.steps.clear();
        if let Some(name) = initial_step {
            self.open_step(name, now_ms);
        }
    }

    /// Fold one activation output line into the step tree.
    ///
    /// `Activating <name>` closes the open step and opens `<name>`; the banner is
    /// swallowed; any other line is appended to the open step (or dropped if none
    /// is open yet, which only happens for home-manager preamble before the first
    /// step).
    pub fn ingest_line(&mut self, line: &str, now_ms: u64) {
        let trimmed = line.trim_end();
        if trimmed == HM_BANNER {
            return;
        }
        // home-manager step names are single identifiers (`_iNote "Activating
        // %s" "<step>"`). Require a single whitespace-free token so a step's own
        // output line that merely starts with "Activating " (e.g. a user script
        // echoing "Activating my service") is treated as content, not a new step
        // that would close the real one early.
        if let Some(name) = trimmed.strip_prefix(ACTIVATING_PREFIX)
            && !name.is_empty()
            && !name.contains(char::is_whitespace)
        {
            self.close_open_step(ActivationStatus::Done, now_ms);
            self.open_step(name.to_owned(), now_ms);
            return;
        }
        if let Some(step) = self.open_step_mut() {
            step.lines.push(line.to_owned());
        }
    }

    /// Settle the activation phase: close the open step as done or failed and set
    /// the terminal status. A failed activation marks the open step (the one that
    /// was running when it died) `Failed`; a clean finish closes it `Done`.
    pub fn finish(&mut self, success: bool, now_ms: u64) {
        let end_status = if success {
            ActivationStatus::Done
        } else {
            ActivationStatus::Failed
        };
        self.close_open_step(end_status, now_ms);
        if success { "done" } else { "failed" }.clone_into(&mut self.status);
    }

    /// Set the phase status without changing steps. Used when a switch skips
    /// activation entirely (e.g. the build failed), so the panel explains why it
    /// stayed empty rather than sitting blank.
    pub fn set_status(&mut self, status: String) {
        self.active = true;
        self.status = status;
    }

    fn open_step(&mut self, name: String, now_ms: u64) {
        self.steps.push(ActivationStep {
            name,
            status: ActivationStatus::Running,
            lines: Vec::new(),
            started_at_ms: now_ms,
            stopped_at_ms: None,
        });
    }

    /// The currently-open (last, still-running) step, if any.
    fn open_step_mut(&mut self) -> Option<&mut ActivationStep> {
        self.steps
            .last_mut()
            .filter(|step| step.status == ActivationStatus::Running)
    }

    fn close_open_step(&mut self, status: ActivationStatus, now_ms: u64) {
        if let Some(step) = self.open_step_mut() {
            step.status = status;
            step.stopped_at_ms = Some(now_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_manager_sequence_builds_named_steps() {
        let mut activation = Activation::default();
        activation.begin("/nix/store/x-home/activate".to_owned(), None, 10);
        // Banner carries no step.
        activation.ingest_line("Starting Home Manager activation", 11);
        assert!(activation.steps.is_empty(), "banner is not a step");

        activation.ingest_line("Activating checkLinkTargets", 20);
        activation.ingest_line("Activating writeBoundary", 30);
        activation.ingest_line("some writeBoundary detail", 31);
        activation.ingest_line("Activating linkGeneration", 40);

        let names: Vec<&str> = activation.steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["checkLinkTargets", "writeBoundary", "linkGeneration"]);

        // The first step closed Done when the next opened.
        assert_eq!(activation.steps[0].status, ActivationStatus::Done);
        assert_eq!(activation.steps[0].stopped_at_ms, Some(30));
        // A non-Activating line appends to the then-open step.
        assert_eq!(activation.steps[1].lines, ["some writeBoundary detail"]);
        // The last step is still running until finish().
        assert_eq!(activation.steps[2].status, ActivationStatus::Running);

        activation.finish(true, 50);
        assert_eq!(activation.steps[2].status, ActivationStatus::Done);
        assert_eq!(activation.steps[2].stopped_at_ms, Some(50));
        assert_eq!(activation.status, "done");
    }

    #[test]
    fn failed_activation_marks_open_step_failed() {
        let mut activation = Activation::default();
        activation.begin("act".to_owned(), None, 0);
        activation.ingest_line("Activating installPackages", 1);
        activation.finish(false, 5);
        assert_eq!(activation.steps[0].status, ActivationStatus::Failed);
        assert_eq!(activation.status, "failed");
    }

    #[test]
    fn unstructured_darwin_stream_lands_under_seeded_step() {
        let mut activation = Activation::default();
        // Darwin: server seeds a single step; lines have no `Activating` markers.
        activation.begin("sudo darwin-rebuild activate".to_owned(), Some("activate".to_owned()), 0);
        activation.ingest_line("setting up /etc...", 1);
        activation.ingest_line("setting up launchd services...", 2);
        assert_eq!(activation.steps.len(), 1);
        assert_eq!(activation.steps[0].name, "activate");
        assert_eq!(
            activation.steps[0].lines,
            ["setting up /etc...", "setting up launchd services..."]
        );
    }

    #[test]
    fn activating_line_with_spaces_is_content_not_a_step_boundary() {
        let mut activation = Activation::default();
        activation.begin("act".to_owned(), None, 0);
        activation.ingest_line("Activating linkGeneration", 1);
        // A step's own output that starts with "Activating " but is a sentence
        // (has whitespace) must not open a new step.
        activation.ingest_line("Activating my custom service now", 2);
        assert_eq!(activation.steps.len(), 1, "no spurious second step");
        assert_eq!(activation.steps[0].name, "linkGeneration");
        assert_eq!(activation.steps[0].lines, ["Activating my custom service now"]);
    }

    #[test]
    fn preamble_before_first_step_is_dropped_for_home() {
        let mut activation = Activation::default();
        activation.begin("act".to_owned(), None, 0);
        // No open step yet: a stray line is dropped rather than inventing a step.
        activation.ingest_line("some preamble", 1);
        assert!(activation.steps.is_empty());
    }
}
