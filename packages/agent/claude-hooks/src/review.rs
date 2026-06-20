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

/// `PostToolUse(Write|Edit|MultiEdit|NotebookEdit)`: record the edited path.
/// Side effect only, never blocks.
pub fn review_log_edit() {
    let Some(input) = crate::read_stdin() else {
        return;
    };
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
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

    // Loop guard: this Stop is already a forced continuation from a prior block.
    // Clear the marker and allow, so the next genuine change-set can trigger again.
    if payload
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&marker);
        return;
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
    use super::safe_session;
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
}
