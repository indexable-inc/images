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
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import TYPE_CHECKING

from .types import Item, SessionRecord, Verdict

if TYPE_CHECKING:
    from .transcripts import Session

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
  - {"op":"delete","id":"<existing id>","reason":"<why retired>"}
  Update an existing item when new evidence refines or contradicts it; add \
only genuinely new lessons. Delete an item ONLY when newer evidence directly \
contradicts it or it is no longer true (a tool/flow changed); prefer update \
over delete, and delete at most a couple per run. Skip sessions that teach \
nothing (trivial chats, aborted runs with no signal). Prefer FEW high-value \
items: at most {max_new} new items this run.
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
    # Raw, unvalidated model output: a JSON list whose entries are validated
    # key-by-key at the merge boundary (apply_operations / session_verdicts),
    # so the element type stays `object` rather than a trusted dict shape.
    operations: list[object]
    session_outcomes: list[object]
    raw: str


class DistillError(RuntimeError):
    pass


def build_prompt(
    project: str,
    items: list[Item],
    digests: list[str],
    max_new: int = 8,
) -> str:
    existing = (
        json.dumps(
            [
                {k: getattr(item, k) for k in ("id", "title", "body", "outcome", "scope")}
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


def _extract_json(text: str) -> dict[str, object]:
    """Parse the model reply, tolerating code fences and stray prose."""
    text = text.strip()
    fenced = re.search(r"```(?:json)?\s*(\{.*\})\s*```", text, re.DOTALL)
    if fenced:
        text = fenced.group(1)
    start = text.find("{")
    end = text.rfind("}")
    if start < 0 or end <= start:
        raise DistillError(f"no JSON object in model reply: {text[:200]!r}")
    parsed = json.loads(text[start : end + 1])
    if not isinstance(parsed, dict):
        raise DistillError(f"model reply is not a JSON object: {text[:200]!r}")
    return parsed


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
            check=False,  # returncode is checked explicitly below
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


def session_verdicts(
    outcomes: Sequence[object], sessions: Sequence[Session]
) -> dict[str, Verdict]:
    """Normalize model verdicts into ``{session_id: {label, reason}}``.

    Exactly one verdict per passed-in session: malformed or off-label model
    entries are dropped, verdicts for session ids the model invented are
    ignored, and unjudged sessions fall back to the scan heuristics so the
    sessions slice never has holes.
    """

    by_id: dict[str, Verdict] = {}
    for verdict in outcomes:
        # Defensive: callers (and run_claude) may pass raw model JSON whose
        # entries are not all objects; skip anything that is not a dict.
        if not isinstance(verdict, dict):
            continue
        sid = verdict.get("session_id")
        label = verdict.get("label")
        if not isinstance(sid, str) or not isinstance(label, str) or label not in SESSION_LABELS:
            continue
        reason = verdict.get("reason")
        by_id[sid] = Verdict(
            label=label,
            reason=_word_clip(reason, 25) if isinstance(reason, str) else "",
        )

    verdicts: dict[str, Verdict] = {}
    for session in sessions:
        judged = by_id.get(session.session_id)
        if judged is not None:
            verdicts[session.session_id] = judged
            continue
        if session.outcome == "unknown" and session.message_count < 6:
            fallback = "abandoned"
        else:
            fallback = _FALLBACK_LABELS.get(session.outcome, "partial")
        verdicts[session.session_id] = Verdict(
            label=fallback,
            reason=f"heuristic fallback (no model verdict; signals: {session.outcome})",
        )
    return verdicts


def _clean_outcome(value: object, default: str) -> str:
    """Coerce an untrusted model ``outcome`` to the closed evidence-label set."""
    if isinstance(value, str) and value in ("success", "failure", "mixed"):
        return value
    return default


def new_item_id() -> str:
    return "df-" + uuid.uuid4().hex[:12]


def apply_operations(
    items: list[Item],
    operations: Sequence[object],
    sessions_meta: dict[str, SessionRecord],
    now: float | None = None,
    id_factory: Callable[[], str] = new_item_id,
    max_new: int = 8,
    max_delete: int = 3,
) -> list[Item]:
    """Merge model operations into the existing item list.

    Items absent from ``operations`` are kept untouched (the anti-collapse
    invariant). ``delete`` ops are capped at ``max_delete`` per run so a
    misbehaving model cannot wipe the playbook. Returns the merged list;
    mutates copies, not the input.
    """

    now = now if now is not None else time.time()
    merged: list[Item] = [item.model_copy() for item in items]
    by_id: dict[str, Item] = {item.id: item for item in merged}
    added = 0
    deleted = 0

    def session_ids(op: dict[str, object]) -> list[str]:
        raw = op.get("sessions")
        if not isinstance(raw, list):
            return []
        return [s for s in raw if isinstance(s, str)][:8]

    for op in operations:
        # Untrusted model JSON: skip any entry that is not an object.
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
            if any(item.title.strip().lower() == title.strip().lower() for item in merged):
                continue  # near-duplicate guard
            outcome = op.get("outcome")
            new_item = Item(
                id=id_factory(),
                title=title.strip(),
                body=_word_clip(body),
                outcome=_clean_outcome(outcome, default="mixed"),
                scope="user" if op.get("scope") == "user" else "shared",
                sessions=session_ids(op),
                first_seen=now,
                last_updated=now,
            )
            merged.append(new_item)
            by_id[new_item.id] = new_item
            added += 1
        elif kind == "update":
            op_id = op.get("id")
            if not isinstance(op_id, str):
                continue
            target = by_id.get(op_id)
            if target is None:
                continue
            new_title = op.get("title")
            if isinstance(new_title, str) and new_title.strip():
                target.title = new_title.strip()
            new_body = op.get("body")
            if isinstance(new_body, str) and new_body.strip():
                target.body = _word_clip(new_body)
            new_outcome = op.get("outcome")
            if isinstance(new_outcome, str) and new_outcome in ("success", "failure", "mixed"):
                target.outcome = new_outcome
            new_scope = op.get("scope")
            if isinstance(new_scope, str) and new_scope in ("user", "shared"):
                target.scope = new_scope
            new_sessions = [s for s in session_ids(op) if s not in target.sessions]
            target.sessions = target.sessions + new_sessions
            target.last_updated = now
        elif kind == "delete":
            if deleted >= max_delete:
                continue
            op_id = op.get("id")
            if not isinstance(op_id, str) or by_id.pop(op_id, None) is None:
                continue  # unknown id is a no-op
            merged = [it for it in merged if it.id != op_id]
            deleted += 1

    # Stamp date ranges from session metadata so provenance survives.
    for item in merged:
        stamps = [
            ts
            for s in item.sessions
            if s in sessions_meta and (ts := sessions_meta[s].last_ts) is not None
        ]
        if stamps:
            prior_from = item.evidence_from
            item.evidence_from = min(stamps + ([prior_from] if prior_from else []))
            prior_to = item.evidence_to
            item.evidence_to = max(stamps + ([prior_to] if prior_to else []))
    return merged


def retire_stale(
    items: list[Item],
    now: float | None = None,
    max_age_days: float = 90.0,
) -> list[Item]:
    """Drop items not evidenced within ``max_age_days`` (deterministic forgetting).

    Recency is the newest evidence timestamp (``evidence_to``), falling back to
    ``last_updated`` for items predating provenance stamps. An item that keeps
    getting re-evidenced stays; one that has gone quiet past the window is
    retired. The window is generous so this is a slow garbage-collector, not an
    aggressive pruner; the model-driven ``delete`` op handles active contradiction.
    """

    now = now if now is not None else time.time()
    cutoff = now - max_age_days * 86400

    def recency(item: Item) -> float | None:
        # The freshest signal wins: an `update` op bumps last_updated even when it
        # carried no in-window session to refresh evidence_to, and such an item
        # must still count as re-evidenced (don't short-circuit on a stale
        # evidence_to). ``None`` means no provenance stamp at all -- a legacy item
        # (pre-provenance shape, also what the migration path loads) whose age is
        # unknown; never retire it on a missing signal, only on a known-stale one,
        # so migrating an old corpus does not wipe its items on the first run.
        return max((t for t in (item.evidence_to, item.last_updated) if t), default=None)

    return [item for item in items if (r := recency(item)) is None or r >= cutoff]
