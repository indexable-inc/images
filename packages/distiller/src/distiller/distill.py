"""Incremental ReasoningBank-style distillation via headless ``claude -p``.

The model is never asked to regenerate the lesson set. It receives the
existing items plus new session evidence and returns a JSON list of
*operations* (``add`` / ``update``); items it does not mention are kept
verbatim. This is the ACE prescription: incremental itemized playbook
updates, never wholesale rewrites (brevity bias / context collapse).
"""

from __future__ import annotations

import json
import re
import subprocess
import tempfile
import time
import uuid
from dataclasses import dataclass

DEFAULT_MODEL = "claude-haiku-4-5-20251001"

# The distiller's own `claude -p` calls land in ~/.claude/projects like any
# session; this prefix lets the scanner drop them so the pipeline never
# distills itself (the recursion otherwise shows up on the very next run).
PROMPT_SENTINEL = "You maintain a small playbook"

_PROMPT_HEADER = """\
You maintain a small playbook of strategy-level lessons distilled from one \
developer's Claude Code sessions in one repository, in the style of \
ReasoningBank: each item is ONE self-contained, reusable lesson or fact that \
would help a future agent working in this repo act faster or avoid a repeated \
mistake. Distill from successes (what worked, which exact commands/flows) AND \
from failures and user corrections (guardrails: what to avoid, what the user \
had to correct).

Rules:
- Each item: a short imperative `title` and a `body` of at most 120 words. \
The body must stand alone (name exact commands, files, conventions); never \
write vague advice like "be careful" or "communicate clearly".
- `outcome` is the evidence label: "success" (worked), "failure" (guardrail \
from something that went wrong), or "mixed".
- `scope` is "user" for personal preference/workflow lessons, "shared" for \
repo/tool facts any agent would benefit from.
- NEVER rewrite the whole playbook. Emit only operations:
  - {"op":"add","title":...,"body":...,"outcome":...,"scope":...,"sessions":[ids]}
  - {"op":"update","id":"<existing id>","title"?:...,"body"?:...,"outcome"?:...,"sessions":[ids]}
  Update an existing item when new evidence refines or contradicts it; add \
only genuinely new lessons. Skip sessions that teach nothing (trivial chats, \
aborted runs with no signal). Prefer FEW high-value items: at most {max_new} \
new items this run.
- `sessions` lists the session ids the evidence came from.
- Separately, judge EVERY session in the new evidence and emit exactly one \
verdict per session id:
  {"session_id":"<id>","label":"success"|"partial"|"failure"|"abandoned","reason":"<one line, at most 25 words>"}
  - "success": the stated goal was accomplished.
  - "partial": real progress, but the goal was not (or not verifiably) finished.
  - "failure": the attempt went wrong (errors piled up, wrong approach, the \
user had to correct or revert).
  - "abandoned": the session fizzled with no real attempt (trivial chat, \
immediate abort).
  Judge from the evidence only; the `outcome-guess` line is a noisy heuristic \
you may overrule.

Respond with ONLY a JSON object: \
{"operations": [...], "session_outcomes": [...]}. No prose.
"""

# Closed label set for per-session outcome verdicts (ENG-2710).
SESSION_LABELS = ("success", "partial", "failure", "abandoned")

# Heuristic Session.outcome -> verdict label, used only when the model
# returned no verdict for a session it was shown.
_FALLBACK_LABELS = {"success": "success", "failure": "failure", "mixed": "partial"}


@dataclass
class DistillResult:
    operations: list[dict]
    session_outcomes: list[dict]
    raw: str


class DistillError(RuntimeError):
    pass


def build_prompt(
    project: str,
    items: list[dict],
    digests: list[str],
    max_new: int = 8,
) -> str:
    existing = (
        json.dumps(
            [
                {k: item[k] for k in ("id", "title", "body", "outcome", "scope")}
                for item in items
            ],
            indent=1,
        )
        if items
        else "[]"
    )
    return "\n".join(
        [
            _PROMPT_HEADER.replace("{max_new}", str(max_new)),
            f"Repository / project: {project}",
            "",
            "## Existing playbook items (do not restate; update by id if needed)",
            existing,
            "",
            "## New session evidence",
            *digests,
        ]
    )


def _envelope_result(envelope: object) -> str:
    """Result text of a ``--output-format json`` envelope.

    Older CLIs print one ``{"result": ...}`` object; current ones (>= 2.1)
    print the full event array whose final entry is ``{"type": "result",
    "result": ...}``. Handle both.
    """

    if isinstance(envelope, dict):
        value = envelope.get("result", "")
        return value if isinstance(value, str) else ""
    if isinstance(envelope, list):
        for event in reversed(envelope):
            if isinstance(event, dict) and event.get("type") == "result":
                value = event.get("result", "")
                return value if isinstance(value, str) else ""
    return ""


def _extract_json(text: str) -> dict:
    """Parse the model reply, tolerating code fences and stray prose."""
    text = text.strip()
    fenced = re.search(r"```(?:json)?\s*(\{.*\})\s*```", text, re.DOTALL)
    if fenced:
        text = fenced.group(1)
    start = text.find("{")
    end = text.rfind("}")
    if start < 0 or end <= start:
        raise DistillError(f"no JSON object in model reply: {text[:200]!r}")
    return json.loads(text[start : end + 1])


def run_claude(
    prompt: str,
    model: str = DEFAULT_MODEL,
    claude_bin: str = "claude",
    timeout: int = 300,
) -> DistillResult:
    """One headless print-mode call; prompt on stdin, JSON envelope on stdout.

    ``--strict-mcp-config`` with an empty config keeps local MCP servers from
    loading (pure text distillation, no tools needed).
    """

    cmd = [
        claude_bin,
        "-p",
        "--model",
        model,
        "--output-format",
        "json",
        "--strict-mcp-config",
        "--mcp-config",
        '{"mcpServers":{}}',
        "--max-turns",
        "2",
    ]
    # Run from a throwaway cwd so the call's own transcript does not land in
    # a real project's directory (it is additionally sentinel-filtered).
    with tempfile.TemporaryDirectory(prefix="ix-distiller-") as scratch:
        proc = subprocess.run(
            cmd,
            input=prompt,
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=scratch,
        )
    if proc.returncode != 0:
        raise DistillError(
            f"claude -p exited {proc.returncode}: {proc.stderr.strip()[:500]}"
        )
    try:
        envelope = json.loads(proc.stdout)
    except json.JSONDecodeError:
        result_text = proc.stdout
    else:
        result_text = _envelope_result(envelope)
    parsed = _extract_json(result_text)
    operations = parsed.get("operations")
    if not isinstance(operations, list):
        raise DistillError(f"model reply lacks an operations list: {result_text[:200]!r}")
    # Outcome verdicts are best-effort: a reply without them still yields
    # lessons, and session_verdicts falls back to the scan heuristics.
    session_outcomes = parsed.get("session_outcomes")
    if not isinstance(session_outcomes, list):
        session_outcomes = []
    return DistillResult(
        operations=operations, session_outcomes=session_outcomes, raw=result_text
    )


def _word_clip(text: str, max_words: int = 120) -> str:
    words = text.split()
    if len(words) <= max_words:
        return text.strip()
    return " ".join(words[:max_words])


def session_verdicts(outcomes: list[dict], sessions: list) -> dict[str, dict]:
    """Normalize model verdicts into ``{session_id: {label, reason}}``.

    Exactly one verdict per passed-in session: malformed or off-label model
    entries are dropped, verdicts for session ids the model invented are
    ignored, and unjudged sessions fall back to the scan heuristics so the
    sessions slice never has holes.
    """

    by_id: dict[str, dict] = {}
    for verdict in outcomes:
        if not isinstance(verdict, dict):
            continue
        sid = verdict.get("session_id")
        label = verdict.get("label")
        if not isinstance(sid, str) or label not in SESSION_LABELS:
            continue
        reason = verdict.get("reason")
        by_id[sid] = {
            "label": label,
            "reason": _word_clip(reason, 25) if isinstance(reason, str) else "",
        }

    verdicts: dict[str, dict] = {}
    for session in sessions:
        judged = by_id.get(session.session_id)
        if judged is not None:
            verdicts[session.session_id] = judged
            continue
        if session.outcome == "unknown" and session.message_count < 6:
            label = "abandoned"
        else:
            label = _FALLBACK_LABELS.get(session.outcome, "partial")
        verdicts[session.session_id] = {
            "label": label,
            "reason": f"heuristic fallback (no model verdict; signals: {session.outcome})",
        }
    return verdicts


def new_item_id() -> str:
    return "df-" + uuid.uuid4().hex[:12]


def apply_operations(
    items: list[dict],
    operations: list[dict],
    sessions_meta: dict[str, dict],
    now: float | None = None,
    id_factory=new_item_id,
    max_new: int = 8,
) -> list[dict]:
    """Merge model operations into the existing item list.

    Items absent from ``operations`` are kept untouched (the anti-collapse
    invariant). Returns the merged list; mutates copies, not the input.
    """

    now = now if now is not None else time.time()
    merged = [dict(item) for item in items]
    by_id = {item["id"]: item for item in merged}
    added = 0

    def session_ids(op: dict) -> list[str]:
        raw = op.get("sessions")
        if not isinstance(raw, list):
            return []
        return [s for s in raw if isinstance(s, str)][:8]

    for op in operations:
        if not isinstance(op, dict):
            continue
        kind = op.get("op")
        if kind == "add":
            if added >= max_new:
                continue
            title = op.get("title")
            body = op.get("body")
            if not isinstance(title, str) or not isinstance(body, str):
                continue
            if any(item["title"].strip().lower() == title.strip().lower() for item in merged):
                continue  # near-duplicate guard
            sessions = session_ids(op)
            item = {
                "id": id_factory(),
                "title": title.strip(),
                "body": _word_clip(body),
                "outcome": op.get("outcome") if op.get("outcome") in ("success", "failure", "mixed") else "mixed",
                "scope": "user" if op.get("scope") == "user" else "shared",
                "sessions": sessions,
                "first_seen": now,
                "last_updated": now,
            }
            merged.append(item)
            by_id[item["id"]] = item
            added += 1
        elif kind == "update":
            item = by_id.get(op.get("id"))
            if item is None:
                continue
            if isinstance(op.get("title"), str) and op["title"].strip():
                item["title"] = op["title"].strip()
            if isinstance(op.get("body"), str) and op["body"].strip():
                item["body"] = _word_clip(op["body"])
            if op.get("outcome") in ("success", "failure", "mixed"):
                item["outcome"] = op["outcome"]
            if op.get("scope") in ("user", "shared"):
                item["scope"] = op["scope"]
            new_sessions = [s for s in session_ids(op) if s not in item.get("sessions", [])]
            item["sessions"] = (item.get("sessions") or []) + new_sessions
            item["last_updated"] = now

    # Stamp date ranges from session metadata so provenance survives.
    for item in merged:
        stamps = [
            sessions_meta[s]["last_ts"]
            for s in item.get("sessions", [])
            if s in sessions_meta and sessions_meta[s].get("last_ts")
        ]
        if stamps:
            item["evidence_from"] = min(
                stamps + ([item["evidence_from"]] if item.get("evidence_from") else [])
            )
            item["evidence_to"] = max(
                stamps + ([item["evidence_to"]] if item.get("evidence_to") else [])
            )
    return merged
