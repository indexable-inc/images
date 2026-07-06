//! Always-on review trigger, the compiled port of the personal `review-log-edit.py`
//! (`PostToolUse` logger) and `review-gate.py` (Stop gate) pair.
//!
//! `review-log-edit` appends each edited path to a per-session marker; `review-gate`
//! reads that marker on Stop and blocks the session once per change-set with a nudge
//! to run the multi-agent `review-changes` skill, then consumes the marker so a
//! review is required at most once per change-set. Both fail OPEN: any parse error,
//! missing field, or bad session id exits silently and lets the session proceed.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

/// Per-session marker dir; overridable so the hooks can be tested against a temp
/// dir (the personal `review-hooks.test.py` contract).
fn state_dir() -> PathBuf {
    std::env::var_os("CLAUDE_REVIEW_STATE_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| crate::home().join(".claude/.review-state"), PathBuf::from)
}

/// `session_id` is interpolated into a file path; accept only a plain filename
/// component so a crafted value cannot escape the state dir.
fn safe_session(payload: &Value) -> Option<String> {
    let session = payload.get("session_id").and_then(Value::as_str)?;
    if session.is_empty() || session == "." || session == ".." || session.contains('/') {
        return None;
    }
    Some(session.to_owned())
}

fn marker_path(session: &str) -> PathBuf {
    state_dir().join(format!("{session}.changed"))
}

/// True when this hook payload fired inside a subagent (Task tool) rather than
/// the main thread. Claude Code populates `agent_id` ONLY inside a subagent call
/// (`PostToolUse` carries it; the docs call it the way to "distinguish subagent
/// hook calls from main-thread calls"), so its presence is the authoritative
/// author signal. A subagent's `PostToolUse` reuses the PARENT `session_id`, so
/// without this check its work-in-progress lands in the parent's marker and the
/// Stop gate blames the main session for edits it never made.
fn is_subagent(payload: &Value) -> bool {
    payload
        .get("agent_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
}

/// `PostToolUse(Write|Edit|MultiEdit|NotebookEdit)`: record the edited path.
/// Side effect only, never blocks.
pub fn review_log_edit() {
    let Some(input) = crate::read_stdin() else {
        return;
    };
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    // Attribute edits to the MAIN session only. A subagent's edits carry the
    // parent's `session_id` but also an `agent_id`; the subagent owns and reviews
    // its own diff, and a still-running background subagent's WIP must not arm the
    // parent's gate. Drop subagent-authored edits here so the gate counts only the
    // main session's own tool calls.
    if is_subagent(&payload) {
        return;
    }
    let Some(session) = safe_session(&payload) else {
        return;
    };
    // Write/Edit/MultiEdit use file_path; NotebookEdit uses notebook_path.
    let Some(path) = payload
        .get("tool_input")
        .and_then(|t| t.get("file_path").or_else(|| t.get("notebook_path")))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let dir = state_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{session}.changed")))
    {
        use std::io::Write as _;
        let _ = writeln!(f, "{path}");
    }
}

#[derive(Serialize)]
struct StopBlock {
    decision: &'static str,
    reason: String,
}

/// True when the turn ended with background work still running. `background_tasks`
/// (Stop/SubagentStop input, v2.1.145+) lists the bash commands and subagents
/// still in flight; the element shape is version-dependent, so presence of any
/// entry is the only signal relied on. A missing or non-array field reads as idle.
fn background_active(payload: &Value) -> bool {
    payload
        .get("background_tasks")
        .and_then(Value::as_array)
        .is_some_and(|tasks| !tasks.is_empty())
}

/// What the Stop gate should do this turn, decided from the payload alone (no I/O).
#[derive(Debug, PartialEq, Eq)]
enum GateAction {
    /// Allow the Stop and clear the marker (loop guard: forced continuation).
    ClearAndAllow,
    /// Allow the Stop but PRESERVE the marker, so the review still fires on a
    /// later Stop. Used while the session's own background work is still running.
    AllowKeepMarker,
    /// Read the marker and, if there are unreviewed edits, consume it and block.
    Evaluate,
}

/// Pure gate policy over the Stop payload. Keeps the two false-positive fixes
/// (loop guard, background-work suppression) testable without stdin/stdout or
/// filesystem state.
fn gate_action(payload: &Value) -> GateAction {
    // Loop guard: this Stop is already a forced continuation from a prior block.
    // Clear the marker and allow, so the next genuine change-set can trigger again.
    if payload
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return GateAction::ClearAndAllow;
    }
    // Don't gate while the main session's own background work is still in flight:
    // a non-empty `background_tasks` means a builder or scaffold subagent is
    // mid-edit. Firing now would blame the parent for work-in-progress and re-fire
    // on every Stop until the builder drains. Allow WITHOUT consuming the marker,
    // so the review still fires on the next genuine Stop once nothing is running.
    if background_active(payload) {
        return GateAction::AllowKeepMarker;
    }
    GateAction::Evaluate
}

/// Stop: block once per change-set if there are unreviewed edits.
pub fn review_gate() {
    let Some(input) = crate::read_stdin() else {
        return;
    };
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    let Some(session) = safe_session(&payload) else {
        return;
    };
    let marker = marker_path(&session);

    match gate_action(&payload) {
        GateAction::ClearAndAllow => {
            let _ = std::fs::remove_file(&marker);
            return;
        }
        GateAction::AllowKeepMarker => return,
        GateAction::Evaluate => {}
    }

    let Ok(contents) = std::fs::read_to_string(&marker) else {
        return;
    };
    let mut files: Vec<String> = contents
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    files.sort_unstable();
    files.dedup();
    if files.is_empty() {
        return;
    }

    // Consume the marker so the review is required at most once per change-set.
    let _ = std::fs::remove_file(&marker);

    let preview = files
        .iter()
        .take(10)
        .map(|p| format!("  - {p}"))
        .collect::<Vec<_>>()
        .join("\n");
    let more = if files.len() > 10 {
        format!("\n  ... and {} more", files.len() - 10)
    } else {
        String::new()
    };
    let reason = format!(
        "Unreviewed edits to {} file(s) this session:\n{preview}{more}\n\n\
         Before finishing, run the `review-changes` skill: it launches a multi-agent \
         review workflow over the working-tree diff (one finder per dimension, then an \
         adversarial verifier per finding). Fix any Correctness or Security blockers it \
         confirms, or say plainly why a finding is declined; Performance and \
         Maintainability are judgment calls.\n\n\
         Skip ONLY if the change is genuinely trivial (a typo, a one-line doc/comment, a \
         config value with no logic): say so in one line and stop. If this turn was a \
         question to the user rather than finished work, restate that question after you \
         dispatch the review.",
        files.len()
    );
    // Stop hook contract: a top-level {"decision":"block","reason":...}, NOT the
    // hookSpecificOutput wrapper the PreToolUse/SessionStart hooks use.
    if let Ok(s) = serde_json::to_string(&StopBlock {
        decision: "block",
        reason,
    }) {
        println!("{s}");
    }
}

#[cfg(test)]
mod tests {
    use super::{GateAction, gate_action, is_subagent, safe_session};
    use serde_json::json;

    #[test]
    fn session_rejects_path_escapes() {
        assert_eq!(safe_session(&json!({"session_id": "abc-123"})).as_deref(), Some("abc-123"));
        assert!(safe_session(&json!({"session_id": "../escape"})).is_none());
        assert!(safe_session(&json!({"session_id": "a/b"})).is_none());
        assert!(safe_session(&json!({"session_id": "."})).is_none());
        assert!(safe_session(&json!({"session_id": ""})).is_none());
        assert!(safe_session(&json!({})).is_none());
    }

    // Attribution: a subagent's PostToolUse reuses the parent `session_id` but
    // carries an `agent_id`. That marks it NOT the main session's own edit, so
    // the logger must drop it and never arm the parent's gate.
    #[test]
    fn subagent_edit_is_attributed_to_subagent() {
        // Main-thread edit: no agent_id.
        assert!(!is_subagent(&json!({
            "session_id": "s1",
            "tool_input": {"file_path": "/repo/src/a.rs"},
        })));
        // Subagent edit: agent_id present (parent session_id reused).
        assert!(is_subagent(&json!({
            "session_id": "s1",
            "agent_id": "agent-xyz",
            "agent_type": "general-purpose",
            "tool_input": {"file_path": "/repo/src/scaffold.rs"},
        })));
        // Empty agent_id is treated as main-thread (defensive).
        assert!(!is_subagent(&json!({"session_id": "s1", "agent_id": ""})));
    }

    #[test]
    fn gate_evaluates_a_plain_main_session_stop() {
        assert_eq!(gate_action(&json!({"session_id": "s1"})), GateAction::Evaluate);
    }

    #[test]
    fn gate_clears_and_allows_on_forced_continuation() {
        // Loop guard: a Stop that is itself a forced continuation clears the marker.
        assert_eq!(
            gate_action(&json!({"session_id": "s1", "stop_hook_active": true})),
            GateAction::ClearAndAllow,
        );
    }

    // Regression: a background subagent (e.g. an in-progress scaffold across many
    // files) is still running when the main turn ends. The gate must NOT fire and
    // must PRESERVE the marker so the review still runs once the builder drains.
    #[test]
    fn gate_defers_while_background_work_runs() {
        assert_eq!(
            gate_action(&json!({
                "session_id": "s1",
                "background_tasks": [
                    {"type": "subagent", "agent_type": "general-purpose", "status": "running"},
                ],
            })),
            GateAction::AllowKeepMarker,
        );
    }

    #[test]
    fn gate_evaluates_when_background_tasks_drained() {
        // Empty or absent background_tasks reads as idle -> normal evaluation.
        assert_eq!(
            gate_action(&json!({"session_id": "s1", "background_tasks": []})),
            GateAction::Evaluate,
        );
        assert_eq!(
            gate_action(&json!({"session_id": "s1", "background_tasks": "notanarray"})),
            GateAction::Evaluate,
        );
    }

    // The loop guard outranks the background-work check: a forced continuation
    // must always clear the marker even if a background task is still listed, so
    // the gate can never wedge into a permanent block.
    #[test]
    fn loop_guard_outranks_background_work() {
        assert_eq!(
            gate_action(&json!({
                "session_id": "s1",
                "stop_hook_active": true,
                "background_tasks": [{"type": "bash", "status": "running"}],
            })),
            GateAction::ClearAndAllow,
        );
    }
}
