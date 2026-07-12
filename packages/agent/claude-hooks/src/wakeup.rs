//! `ScheduleWakeup` drop detector: the `wakeup-log` (`PostToolUse`) and
//! `wakeup-gate` (Stop) pair.
//!
//! A pending `ScheduleWakeup` lives only in harness process memory (it is never
//! written to `scheduled_tasks.json`), a session resume or user abort clears the
//! whole pending list, and the missed-task recovery path covers persisted tasks
//! only. A dropped wakeup therefore silently never fires: in
//! indexable-inc/index#2259 a background session armed a wakeup, four
//! notification-driven turns intervened, the fire time passed with nothing
//! scheduled, and the session idled 24h until nudged.
//!
//! `wakeup-log` records each armed fire time (the tool's `scheduledFor` epoch
//! ms) in a per-session marker. `wakeup-gate` compares that marker against the
//! Stop payload's `session_crons` (present since Claude Code 2.1.x; lists
//! pending `ScheduleWakeup`/`CronCreate`/loop crons): if the recorded fire time is
//! still ahead but no one-shot cron remains pending, the wakeup was dropped and
//! the gate blocks ONCE with a re-arm nudge, turning a silent stall into a loud
//! correction. Like every hook in this crate both fail OPEN and SILENT: missing
//! input, parse errors, an absent `session_crons` field (older harness), or a
//! marker already nagged all allow the Stop.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Per-session marker dir; overridable so the hooks can be tested against a
/// temp dir, matching the `review-gate` `CLAUDE_REVIEW_STATE_DIR` convention.
fn state_dir() -> PathBuf {
    std::env::var_os("CLAUDE_WAKEUP_STATE_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| crate::home().join(".claude/.wakeup-state"), PathBuf::from)
}

fn marker_path(session: &str) -> PathBuf {
    state_dir().join(format!("{session}.wakeup.json"))
}

/// The armed wakeup this session is counting on. `nagged` caps the gate at one
/// block per armed wakeup so a model that deliberately ends the loop is never
/// wedged into a permanent block.
#[derive(Serialize, Deserialize)]
struct Marker {
    scheduled_for_ms: i64,
    armed_at_ms: i64,
    nagged: bool,
}

fn read_marker(session: &str) -> Option<Marker> {
    let text = std::fs::read_to_string(marker_path(session)).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_marker(session: &str, marker: &Marker) {
    let dir = state_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(s) = serde_json::to_string(marker) {
        let _ = std::fs::write(marker_path(session), s);
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// `PostToolUse(ScheduleWakeup)`: record the armed fire time. The tool's
/// structured response carries `scheduledFor` (epoch ms of the registered fire
/// target, already clamped and minute-rounded by the harness); `0` means the
/// loop runtime declined to schedule (gate off or loop over), which clears any
/// stale marker. Side effect only, never blocks.
pub fn wakeup_log() {
    let Some(input) = crate::read_stdin() else {
        return;
    };
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    // ScheduleWakeup is main-thread /loop machinery; a subagent's call (parent
    // session_id reused, `agent_id` set) must not arm the parent's gate.
    if crate::is_subagent(&payload) {
        return;
    }
    let Some(session) = crate::safe_session(&payload) else {
        return;
    };
    let Some(scheduled_for) = payload
        .get("tool_response")
        .and_then(|r| r.get("scheduledFor"))
        .and_then(Value::as_i64)
    else {
        return;
    };
    if scheduled_for <= 0 {
        let _ = std::fs::remove_file(marker_path(&session));
        return;
    }
    write_marker(
        &session,
        &Marker {
            scheduled_for_ms: scheduled_for,
            armed_at_ms: now_ms(),
            nagged: false,
        },
    );
}

/// What the Stop gate should do this turn, decided from payload + marker + the
/// clock alone (no I/O), so the policy is testable like `review::gate_action`.
#[derive(Debug, PartialEq, Eq)]
enum GateAction {
    /// Nothing to say; leave any marker in place.
    Allow,
    /// The marker is spent (fire time reached, or already nagged): remove it
    /// and allow.
    ClearAndAllow,
    /// The armed wakeup vanished before its fire time: block once with a
    /// re-arm nudge and remember the nag.
    Block,
}

fn gate_action(payload: &Value, marker: &Marker, now_ms: i64) -> GateAction {
    // At or past the fire target the Stop cannot distinguish "fired" (the fire
    // starts its own turn) from "dropped", so the marker is spent: fail open.
    if now_ms >= marker.scheduled_for_ms {
        return GateAction::ClearAndAllow;
    }
    // `session_crons` absent (or malformed) means this harness predates the
    // field; there is nothing to verify against, so fail open.
    let Some(crons) = payload.get("session_crons").and_then(Value::as_array) else {
        return GateAction::Allow;
    };
    // Any pending one-shot counts as the wakeup being alive. A distinct
    // one-shot CronCreate task could mask a dropped wakeup here, but that
    // false negative only fails open, and prompts are clipped in the payload,
    // so exact matching would be fragile.
    let alive = crons
        .iter()
        .any(|c| c.get("recurring").and_then(Value::as_bool) == Some(false));
    if alive {
        return GateAction::Allow;
    }
    if marker.nagged {
        return GateAction::ClearAndAllow;
    }
    GateAction::Block
}

#[derive(Serialize)]
struct StopBlock {
    decision: &'static str,
    reason: String,
}

fn fmt_local(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map_or_else(
        || format!("epoch+{ms}ms"),
        |t| {
            t.with_timezone(&chrono::Local)
                .format("%H:%M:%S")
                .to_string()
        },
    )
}

/// Stop: block once when the armed wakeup is gone before its fire time.
pub fn wakeup_gate() {
    let Some(input) = crate::read_stdin() else {
        return;
    };
    let Ok(payload) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    let Some(session) = crate::safe_session(&payload) else {
        return;
    };
    let Some(marker) = read_marker(&session) else {
        return;
    };
    let now = now_ms();
    match gate_action(&payload, &marker, now) {
        GateAction::Allow => return,
        GateAction::ClearAndAllow => {
            let _ = std::fs::remove_file(marker_path(&session));
            return;
        }
        GateAction::Block => {}
    }
    write_marker(&session, &Marker { nagged: true, ..marker });
    let reason = format!(
        "An armed ScheduleWakeup was dropped before firing: this session armed a wakeup at {} \
         to fire at {} (in {}s), but the harness now reports no pending one-shot wakeup in \
         `session_crons`, so nothing will re-invoke this session \
         (indexable-inc/index#2259: session resume and user abort silently clear pending \
         wakeups). If the loop should continue, call ScheduleWakeup again now to re-arm. If \
         the loop is deliberately over, or the user cancelled it, finish the turn without one.",
        fmt_local(marker.armed_at_ms),
        fmt_local(marker.scheduled_for_ms),
        (marker.scheduled_for_ms - now) / 1000,
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
    use super::{GateAction, Marker, gate_action};
    use serde_json::json;

    fn marker(scheduled_for_ms: i64, nagged: bool) -> Marker {
        Marker {
            scheduled_for_ms,
            armed_at_ms: scheduled_for_ms - 3_000_000,
            nagged,
        }
    }

    // The #2259 shape: fire time still ahead, but the pending wakeup vanished
    // from `session_crons` after intervening turns. Must block.
    #[test]
    fn blocks_when_pending_wakeup_vanished_before_fire_time() {
        assert_eq!(
            gate_action(&json!({"session_crons": []}), &marker(2_000_000, false), 1_000_000),
            GateAction::Block,
        );
    }

    #[test]
    fn allows_while_wakeup_still_pending() {
        let payload = json!({"session_crons": [
            {"id": "ab12", "schedule": "20 19 * * *", "recurring": false, "prompt": "/loop ..."},
        ]});
        assert_eq!(gate_action(&payload, &marker(2_000_000, false), 1_000_000), GateAction::Allow);
    }

    // A recurring cron (CronCreate autonomous loop) is not the one-shot wakeup;
    // its presence must not mask a dropped ScheduleWakeup.
    #[test]
    fn recurring_cron_does_not_count_as_alive() {
        let payload = json!({"session_crons": [
            {"id": "cd34", "schedule": "0 9 * * *", "recurring": true, "prompt": "daily"},
        ]});
        assert_eq!(gate_action(&payload, &marker(2_000_000, false), 1_000_000), GateAction::Block);
    }

    // At or past the fire target the fire itself starts a turn, so the Stop
    // cannot distinguish fired from dropped: the marker is spent.
    #[test]
    fn clears_once_fire_time_passed() {
        assert_eq!(
            gate_action(&json!({"session_crons": []}), &marker(2_000_000, false), 2_000_000),
            GateAction::ClearAndAllow,
        );
    }

    // Loop guard: one nag per armed wakeup. A model that deliberately ends the
    // loop after the nudge must not be wedged into a permanent block.
    #[test]
    fn blocks_at_most_once_per_armed_wakeup() {
        assert_eq!(
            gate_action(&json!({"session_crons": []}), &marker(2_000_000, true), 1_000_000),
            GateAction::ClearAndAllow,
        );
    }

    // An older harness without `session_crons` gives nothing to verify: fail
    // open and keep the marker (it clears itself once the fire time passes).
    #[test]
    fn missing_session_crons_fails_open() {
        assert_eq!(gate_action(&json!({}), &marker(2_000_000, false), 1_000_000), GateAction::Allow);
        assert_eq!(
            gate_action(&json!({"session_crons": "notanarray"}), &marker(2_000_000, false), 1_000_000),
            GateAction::Allow,
        );
    }
}
