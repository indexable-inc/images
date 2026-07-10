#!/usr/bin/env python3
# ruff: noqa
"""Unit test for the always-on review hooks (review-log-edit.py + review-gate.py).

Drives both scripts as subprocesses against a temp state dir (via
CLAUDE_REVIEW_STATE_DIR) with fixture stdin, and asserts the gate blocks exactly
once per change-set and never loops. Run: python3 review-hooks.test.py
"""
import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
LOG = os.path.join(HERE, "review-log-edit.py")
GATE = os.path.join(HERE, "review-gate.py")
SID = "test-session-123"


def run(script, payload, state_dir):
    env = dict(os.environ, CLAUDE_REVIEW_STATE_DIR=state_dir)
    proc = subprocess.run(
        [sys.executable, script],
        input=json.dumps(payload),
        capture_output=True,
        text=True,
        env=env,
    )
    return proc.returncode, proc.stdout.strip()


def log_edit(state_dir, path, tool="Write", key="file_path"):
    return run(LOG, {"session_id": SID, "tool_name": tool, "tool_input": {key: path}}, state_dir)


def gate(state_dir, stop_hook_active=False):
    return run(GATE, {"session_id": SID, "stop_hook_active": stop_hook_active}, state_dir)


def marker(state_dir):
    return os.path.join(state_dir, f"{SID}.changed")


failures = []


def check(name, cond):
    print(("ok   " if cond else "FAIL ") + name)
    if not cond:
        failures.append(name)


with tempfile.TemporaryDirectory() as d:
    # 1. No edits -> gate allows (exit 0, no JSON).
    code, out = gate(d)
    check("gate allows when no marker", code == 0 and out == "")

    # 2. An edit is logged, then a NotebookEdit -> gate blocks once.
    log_edit(d, "/repo/src/a.py")
    log_edit(d, "/repo/nb.ipynb", tool="NotebookEdit", key="notebook_path")
    code, out = gate(d)
    blocked = code == 0 and out != "" and json.loads(out).get("decision") == "block"
    reason = json.loads(out).get("reason", "") if out else ""
    check("gate blocks after edits", blocked)
    check("block reason names review-changes skill", "review-changes" in reason)
    check("block reason counts 2 files", "2 file(s)" in reason)
    check("marker consumed after block", not os.path.exists(marker(d)))

    # 3. Forced-continuation Stop (stop_hook_active) -> always allows (loop guard).
    log_edit(d, "/repo/src/b.py")
    code, out = gate(d, stop_hook_active=True)
    check("loop guard allows on stop_hook_active", code == 0 and out == "")
    check("loop guard clears marker", not os.path.exists(marker(d)))

    # 4. Edits without file_path (e.g. a non-edit tool) are ignored by the logger.
    run(LOG, {"session_id": SID, "tool_name": "Bash", "tool_input": {"command": "ls"}}, d)
    code, out = gate(d)
    check("non-edit tool does not arm the gate", code == 0 and out == "")

    # 5. A path-traversal session_id is rejected by both hooks (no escape).
    run(LOG, {"session_id": "../escape", "tool_name": "Write", "tool_input": {"file_path": "/x"}}, d)
    escaped = os.path.join(os.path.dirname(d), "escape.changed")
    code, out = run(GATE, {"session_id": "../escape", "stop_hook_active": False}, d)
    check("traversal session_id writes no marker outside state dir", not os.path.exists(escaped))
    check("gate allows on traversal session_id", code == 0 and out == "")

    # 6. Non-dict JSON stdin is handled (gate allows, no crash).
    code, out = run(GATE, [], d)
    check("gate allows on non-dict stdin", code == 0 and out == "")

if failures:
    print(f"\n{len(failures)} FAILED: {failures}")
    sys.exit(1)
print("\nall passed")
