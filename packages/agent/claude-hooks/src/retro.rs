//! Always-on session-retrospective trigger, a sibling of the `review-gate` Stop
//! hook (`review.rs`). On Stop of a substantive session it blocks ONCE with a
//! nudge to run the `session-retro` skill, which walks the finished session and
//! files GitHub issues for everything improvable. A per-session marker makes it
//! fire at most once per session, mirroring how `review-gate` tracks its state.
//!
//! Substantive-session heuristic: a session that made enough tool calls is worth
//! retrospecting; a trivial one-question session is not. The count comes from the
//! Stop payload's `transcript_path` (both the Claude and codex JSONL dialects
//! carry `tool_use` / `function_call` entries), gated by [`RETRO_MIN_TOOL_CALLS`].
//!
//! Like every hook in this crate it fails OPEN and SILENT: any missing input,
//! parse error, or bad session id exits quietly and never blocks Stop. It shares
//! the loop-guard and background-work suppression policy with `review-gate` so a
//! forced continuation can never wedge it into a permanent block.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

/// Below this many tool calls a session is a trivial one-question interaction not
/// worth a retro. Overridable for tests and tuning.
const DEFAULT_MIN_TOOL_CALLS: usize = 8;

/// Per-session marker dir; overridable so the hook can be tested against a temp
/// dir, matching the `review-gate` `CLAUDE_REVIEW_STATE_DIR` convention.
fn state_dir() -> PathBuf {
    std::env::var_os("CLAUDE_RETRO_STATE_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| crate::home().join(".claude/.retro-state"), PathBuf::from)
}

fn min_tool_calls() -> usize {
    std::env::var("CLAUDE_RETRO_MIN_TOOL_CALLS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(DEFAULT_MIN_TOOL_CALLS)
}

/// `session_id` is interpolated into a file path; accept only a plain filename
/// component so a crafted value cannot escape the state dir. Same contract as
/// `review::safe_session`.
fn safe_session(payload: &Value) -> Option<String> {
    let session = payload.get("session_id").and_then(Value::as_str)?;
    if session.is_empty() || session == "." || session == ".." || session.contains('/') {
        return None;
    }
    Some(session.to_owned())
}

/// One `<session>.retro-done` marker per session: its presence means the retro
/// gate already fired, so a later Stop must not fire again.
fn marker_path(session: &str) -> PathBuf {
    state_dir().join(format!("{session}.retro-done"))
}

/// What the Stop gate should do this turn, decided from the payload alone (no
/// transcript I/O). Mirrors `review::GateAction`.
#[derive(Debug, PartialEq, Eq)]
enum GateAction {
    /// This Stop is a forced continuation from a prior block: allow so the loop
    /// can never wedge.
    Allow,
    /// The main session's own background work is still running: allow now so the
    /// retro fires on a later Stop once nothing is in flight.
    Defer,
    /// Read the transcript, and if substantive and not already done, fire.
    Evaluate,
}

/// True when the turn ended with the session's own background work still
/// running. Same `background_tasks` signal `review-gate` uses.
fn background_active(payload: &Value) -> bool {
    payload
        .get("background_tasks")
        .and_then(Value::as_array)
        .is_some_and(|tasks| !tasks.is_empty())
}

/// Pure gate policy over the Stop payload.
fn gate_action(payload: &Value) -> GateAction {
    if payload
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return GateAction::Allow;
    }
    if background_active(payload) {
        return GateAction::Defer;
    }
    GateAction::Evaluate
}

/// Count tool calls in a session transcript, across both dialects. Claude JSONL
/// carries `{"type":"tool_use",...}` content items on assistant messages; the
/// codex rollout carries `{"type":"function_call",...}` payload items. Counting
/// any occurrence of either type token is a cheap, dialect-agnostic proxy: a
/// deliberate over-count of a nested field is still bounded by transcript size
/// and only nudges the substantive threshold, never blocks Stop.
fn count_tool_calls(transcript: &str) -> usize {
    transcript
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .map(|v| count_tool_calls_in_value(&v))
        .sum()
}

/// Recursively count objects whose `type` is a tool-call marker.
fn count_tool_calls_in_value(v: &Value) -> usize {
    match v {
        Value::Object(map) => {
            let here = usize::from(
                map.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t == "tool_use" || t == "function_call"),
            );
            here + map.values().map(count_tool_calls_in_value).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(count_tool_calls_in_value).sum(),
        _ => 0,
    }
}

/// A session is substantive when it made at least the threshold of tool calls.
const fn is_substantive(tool_calls: usize, threshold: usize) -> bool {
    tool_calls >= threshold
}

#[derive(Serialize)]
struct StopBlock {
    decision: &'static str,
    reason: String,
}

/// Stop: block once per substantive session with a nudge to run `session-retro`.
pub fn retro_gate() {
    if crate::flag_set("CLAUDE_CODE_DISABLE_RETRO_GATE") {
        return;
    }
    let Some(input) = crate::read_stdin() else {
        return;
    };
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    let Some(session) = safe_session(&payload) else {
        return;
    };

    match gate_action(&payload) {
        // A forced continuation still marks the retro done, so a later plain Stop
        // does not re-fire after the skill (or a decline) has run.
        GateAction::Allow => {
            let dir = state_dir();
            if std::fs::create_dir_all(&dir).is_ok() {
                let _ = std::fs::write(marker_path(&session), b"");
            }
            return;
        }
        GateAction::Defer => return,
        GateAction::Evaluate => {}
    }

    let marker = marker_path(&session);
    if marker.exists() {
        return;
    }

    let Some(transcript) = payload.get("transcript_path").and_then(Value::as_str) else {
        return;
    };
    if transcript.is_empty() || !Path::new(transcript).is_file() {
        return;
    }
    let Ok(contents) = std::fs::read_to_string(transcript) else {
        return;
    };
    if !is_substantive(count_tool_calls(&contents), min_tool_calls()) {
        return;
    }

    // Mark done BEFORE blocking: the block forces a continuation whose Stop must
    // not fire the gate again. Losing the block to a write failure beats a wedge.
    let dir = state_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if std::fs::write(&marker, b"").is_err() {
        return;
    }

    let reason = "Before finishing, run the `session-retro` skill: walk this \
         session for everything improvable (corrected mistakes, denied or guarded \
         tool calls, workarounds, missing structured interfaces, hook noise, \
         stalled watches, anything repeated), route each to the owning repo, \
         dedupe against open issues, and file concise GitHub issues per the \
         `issues` skill with AI attribution. Bias to filing; skip only real \
         duplicates. If this turn was a question to the user rather than finished \
         work, restate that question after you dispatch the retro."
        .to_owned();

    // Stop hook contract: top-level {"decision":"block","reason":...}.
    if let Ok(s) = serde_json::to_string(&StopBlock {
        decision: "block",
        reason,
    }) {
        println!("{s}");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GateAction, count_tool_calls, gate_action, is_substantive, safe_session,
    };
    use serde_json::json;

    #[test]
    fn session_rejects_path_escapes() {
        assert_eq!(
            safe_session(&json!({"session_id": "abc-123"})).as_deref(),
            Some("abc-123")
        );
        assert!(safe_session(&json!({"session_id": "../escape"})).is_none());
        assert!(safe_session(&json!({"session_id": "a/b"})).is_none());
        assert!(safe_session(&json!({"session_id": "."})).is_none());
        assert!(safe_session(&json!({"session_id": ""})).is_none());
        assert!(safe_session(&json!({})).is_none());
    }

    #[test]
    fn gate_evaluates_a_plain_main_session_stop() {
        assert_eq!(
            gate_action(&json!({"session_id": "s1"})),
            GateAction::Evaluate
        );
    }

    #[test]
    fn gate_allows_on_forced_continuation() {
        // Loop guard: a forced continuation must always allow so the gate can
        // never wedge into a permanent block.
        assert_eq!(
            gate_action(&json!({"session_id": "s1", "stop_hook_active": true})),
            GateAction::Allow,
        );
    }

    #[test]
    fn gate_defers_while_background_work_runs() {
        assert_eq!(
            gate_action(&json!({
                "session_id": "s1",
                "background_tasks": [{"type": "subagent", "status": "running"}],
            })),
            GateAction::Defer,
        );
    }

    #[test]
    fn loop_guard_outranks_background_work() {
        assert_eq!(
            gate_action(&json!({
                "session_id": "s1",
                "stop_hook_active": true,
                "background_tasks": [{"type": "bash", "status": "running"}],
            })),
            GateAction::Allow,
        );
    }

    #[test]
    fn counts_tool_calls_across_dialects() {
        // Claude dialect: tool_use content items on assistant messages.
        let claude = [
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"ok"},{"type":"tool_use","name":"Bash"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","name":"Read"}]}}"#,
            // user message with no tool call
            r#"{"type":"user","message":{"role":"user","content":"hi"}}"#,
        ]
        .join("\n");
        assert_eq!(count_tool_calls(&claude), 2);

        // Codex dialect: function_call payload items.
        let codex = [
            r#"{"payload":{"type":"function_call","name":"shell"}}"#,
            r#"{"payload":{"type":"function_call","name":"apply_patch"}}"#,
        ]
        .join("\n");
        assert_eq!(count_tool_calls(&codex), 2);
    }

    #[test]
    fn substantive_threshold() {
        assert!(!is_substantive(7, 8));
        assert!(is_substantive(8, 8));
        assert!(is_substantive(20, 8));
    }
}
