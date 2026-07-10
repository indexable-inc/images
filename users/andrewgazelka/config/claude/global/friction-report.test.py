#!/usr/bin/env python3
# ruff: noqa
"""Unit test for friction-report.py (the Stop friction-mining hook).

Drives the hook as a subprocess with FRICTION_FOREGROUND=1 against a temp
state dir, a stub `claude` (captures its stdin, prints a canned response) and
a local HTTP server standing in for Linear (captures issueCreate bodies).
Run: python3 friction-report.test.py
"""
import http.server
import json
import os
import subprocess
import sys
import tempfile
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
HOOK = os.path.join(HERE, "friction-report.py")
SID = "fric-session-1"

failures = []


def check(name, cond):
    print(("ok   " if cond else "FAIL ") + name)
    if not cond:
        failures.append(name)


# --- stub Linear endpoint -----------------------------------------------------
posts = []


class Linear(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        posts.append({"auth": self.headers.get("Authorization"), "body": body})
        n = len(posts)
        out = json.dumps(
            {"data": {"issueCreate": {"success": True, "issue": {"identifier": f"ENG-{n}", "url": f"https://linear.app/x/ENG-{n}"}}}}
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)

    def log_message(self, *a):
        pass


server = http.server.HTTPServer(("127.0.0.1", 0), Linear)
threading.Thread(target=server.serve_forever, daemon=True).start()
LINEAR_URL = f"http://127.0.0.1:{server.server_port}/graphql"


def claude_line(role, text):
    return json.dumps({"type": role, "message": {"role": role, "content": [{"type": "text", "text": text}]}})


def run_hook(tmp, payload, response):
    """Run the hook foreground with a stub claude that prints `response`."""
    stub_dir = os.path.join(tmp, "stub")
    os.makedirs(stub_dir, exist_ok=True)
    capture = os.path.join(stub_dir, "claude-stdin.txt")
    stub = os.path.join(stub_dir, "claude")
    with open(stub, "w") as f:
        f.write(
            "#!/bin/sh\n"
            f"cat > {capture}\n"
            f"cat {os.path.join(stub_dir, 'response.json')}\n"
        )
    os.chmod(stub, 0o755)
    with open(os.path.join(stub_dir, "response.json"), "w") as f:
        f.write(response)
    if os.path.exists(capture):
        os.remove(capture)
    env = dict(
        os.environ,
        FRICTION_FOREGROUND="1",
        FRICTION_STATE_DIR=os.path.join(tmp, "state"),
        FRICTION_CLAUDE_CMD=stub,
        FRICTION_LINEAR_URL=LINEAR_URL,
        FRICTION_LINEAR_KEY="test-key",
        FRICTION_MIN_DELTA="10",
    )
    proc = subprocess.run(
        [sys.executable, HOOK], input=json.dumps(payload), capture_output=True, text=True, env=env
    )
    seen = open(capture).read() if os.path.exists(capture) else None
    return proc, seen


TWO_ITEMS = json.dumps(
    [
        {"kind": "user-intervention", "title": "User had to re-explain the deploy flow", "description": "The agent guessed; CLAUDE.md should document it."},
        {"kind": "weak-tool", "title": "grep tool truncated output silently", "description": "Needed a limit flag; agent retried four times."},
    ]
)

with tempfile.TemporaryDirectory() as tmp:
    transcript = os.path.join(tmp, "transcript.jsonl")
    with open(transcript, "w") as f:
        f.write(claude_line("user", "no, stop - use the OTHER api, I told you this before") + "\n")
        f.write(claude_line("assistant", "Sorry, switching to the other API now.") + "\n")
    payload = {"session_id": SID, "transcript_path": transcript, "cwd": "/repo"}

    # 1. First stop: whole transcript analyzed, both items filed.
    proc, seen = run_hook(tmp, payload, TWO_ITEMS)
    check("exit 0", proc.returncode == 0)
    check("model saw the user intervention", seen is not None and "OTHER api" in seen)
    check("two issues filed", len(posts) == 2)
    if len(posts) == 2:
        inp = posts[0]["body"]["variables"]["input"]
        check("raw key auth (not Bearer)", posts[0]["auth"] == "test-key")
        check("team is ENG", inp["teamId"] == "a8845362-21c7-4283-ba80-cea987a3ee74")
        check("project is Shitty", inp["projectId"] == "acfc01e7-7246-4ebb-91f5-6d5bb8d1c476")
        check("description carries AI attribution", "sent by an AI agent" in inp["description"])
        check("description carries kind + session", "`user-intervention`" in inp["description"] and SID in inp["description"])

    # 2. Second stop, nothing new in the transcript: no model call, no filing.
    proc, seen = run_hook(tmp, payload, TWO_ITEMS)
    check("no re-analysis without new content", seen is None and len(posts) == 2)

    # 3. New turn appended: only the delta reaches the model; duplicate title
    #    is not re-filed, new title is.
    with open(transcript, "a") as f:
        f.write(claude_line("user", "why is the build cache empty again??") + "\n")
        f.write(claude_line("assistant", "Investigating the cache.") + "\n")
    second = json.dumps(
        [
            {"kind": "weak-tool", "title": "grep tool truncated output silently", "description": "dupe"},
            {"kind": "missing-context", "title": "Build cache lifecycle undocumented", "description": "Cache emptied unexpectedly."},
        ]
    )
    proc, seen = run_hook(tmp, payload, second)
    check("delta only (old content not resent)", seen is not None and "build cache" in seen and "OTHER api" not in seen)
    check("dupe title skipped, new one filed", len(posts) == 3 and "cache" in posts[2]["body"]["variables"]["input"]["title"].lower())

    # 4. Garbage model output: no filing, no crash, offset still advances.
    with open(transcript, "a") as f:
        f.write(claude_line("user", "another turn with enough text to clear the minimum delta") + "\n")
    proc, seen = run_hook(tmp, payload, "I could not find anything of note!")
    check("garbage output files nothing", proc.returncode == 0 and len(posts) == 3)
    proc, seen = run_hook(tmp, payload, TWO_ITEMS)
    check("garbage run still advanced the offset", seen is None and len(posts) == 3)

    # 5. Tool errors and codex-style lines are condensed; meta lines are not.
    with open(transcript, "a") as f:
        f.write(json.dumps({"type": "user", "isMeta": True, "message": {"role": "user", "content": [{"type": "text", "text": "META-MARKER"}]}}) + "\n")
        f.write(json.dumps({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "is_error": True, "content": "exploded: ENOENT"}]}}) + "\n")
        f.write(json.dumps({"type": "event_msg", "payload": {"type": "user_message", "message": "codex user says hi"}}) + "\n")
        f.write(claude_line("user", "padding so the delta clears the minimum length easily") + "\n")
    proc, seen = run_hook(tmp, payload, "[]")
    check("tool error labeled", seen is not None and "TOOL ERROR: " in seen and "ENOENT" in seen)
    check("codex user_message extracted", seen is not None and "codex user says hi" in seen)
    check("meta line skipped", seen is not None and "META-MARKER" not in seen)

    # 6. Hostile/invalid payloads are no-ops.
    for name, bad in [
        ("traversal session_id", {"session_id": "../evil", "transcript_path": transcript}),
        ("missing transcript", {"session_id": SID, "transcript_path": os.path.join(tmp, "nope.jsonl")}),
        ("non-dict stdin", []),
    ]:
        proc, _ = run_hook(tmp, bad, TWO_ITEMS)
        check(f"{name} is a clean no-op", proc.returncode == 0 and len(posts) == 3)
    evil_state = os.path.join(tmp, "state", "..", "evil.json")
    check("no state written outside the dir", not os.path.exists(os.path.normpath(evil_state)))

server.shutdown()
if failures:
    print(f"\n{len(failures)} FAILED: {failures}")
    sys.exit(1)
print("\nall passed")
