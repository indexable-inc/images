"""Human-readable rendering of the agent CLI's stream-json (index#3496).

The tmux pane a ``fabric.claude.session`` CLI runs in (:mod:`fabric.tmux`)
used to show the CLI's raw stream-json: machine framing, not something a
human can follow. This module is the pane's display side:
``Transcript.feed`` takes one raw stdout line and returns the text to
show, so the pane reads as a live transcript (assistant text, tool calls
with compact inputs, truncated tool results, a final result summary)
while the SDK keeps the raw bytes through its own FIFO.

Rendering never swallows information silently: a line that is not JSON,
or a JSON event this module does not know, is shown as-is (unknown JSON
dimmed and clipped), so a CLI format change degrades to the old raw view
rather than a blank pane. Only frames that are protocol traffic, not
conversation (SDK<->CLI control frames, ``stream_event`` partial deltas
that a complete message always follows), render as nothing.
"""

from __future__ import annotations

import json

__all__ = ["Transcript"]

# ANSI SGR fragments; the pane is a tmux terminal, so ANSI always applies.
_RESET = "\x1b[0m"
_DIM = "\x1b[2m"
_BOLD = "\x1b[1m"
_CYAN = "\x1b[36m"
_GREEN = "\x1b[32m"
_RED = "\x1b[31m"

# SDK<->CLI protocol frames, never conversation content.
_CONTROL_TYPES = frozenset({"control_request", "control_response", "control_cancel_request"})

# Clipping budgets: a tool input can be a whole file write and a tool
# result a whole build log; the pane wants the shape, the journal keeps
# the payload (fabric.claude records every raw message to CAS).
_INPUT_LIMIT = 200
_THINKING_LIMIT = 200
_UNKNOWN_LIMIT = 400
_RESULT_LINES = 4
_RESULT_LINE_LIMIT = 200


def _clip(text: str, limit: int) -> str:
    return text if len(text) <= limit else text[: limit - 1] + "\u2026"


def _compact(value: object, limit: int) -> str:
    """One clipped line of JSON, for tool inputs and unknown payloads."""

    return _clip(json.dumps(value, default=repr), limit)


def _tool_result_text(content: object) -> str:
    """Flatten a tool_result ``content`` (str | block list | None) to text."""

    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts: list[str] = []
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                parts.append(str(block.get("text", "")))
            else:
                parts.append(json.dumps(block, default=repr))
        return "\n".join(parts)
    return json.dumps(content, default=repr)


def _excerpt(text: str) -> list[str]:
    """The first few clipped lines of ``text``, with a ``+N lines`` tail."""

    lines = text.splitlines() or [""]
    shown = [_clip(line, _RESULT_LINE_LIMIT) for line in lines[:_RESULT_LINES]]
    hidden = len(lines) - len(shown)
    if hidden > 0:
        shown.append(f"\u2026 +{hidden} lines")
    return shown


class Transcript:
    """Stateful line renderer: one raw stream-json line in, pane text out.

    ``feed`` returns the text to display for that line (may span several
    display lines), or ``None`` for protocol frames with nothing to show.
    State is one map from tool_use id to tool name, so a later
    tool_result can be labeled with the call it answers.
    """

    def __init__(self) -> None:
        self._tool_names: dict[str, str] = {}

    def feed(self, raw: bytes) -> str | None:
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            return None
        try:
            event = json.loads(line)
        except ValueError:
            return line  # not stream-json: show verbatim
        if not isinstance(event, dict):
            return line
        kind = event.get("type")
        if kind in _CONTROL_TYPES or kind == "stream_event":
            return None
        if kind == "system":
            return self._system(event)
        if kind in ("assistant", "user"):
            return self._message(event)
        if kind == "result":
            return self._result(event)
        return f"{_DIM}{_clip(line, _UNKNOWN_LIMIT)}{_RESET}"

    def _system(self, event: dict[str, object]) -> str:
        subtype = event.get("subtype") or "system"
        if subtype == "init":
            fields = " ".join(
                f"{key}={event[key]}"
                for key in ("model", "permissionMode", "cwd", "session_id")
                if event.get(key)
            )
            return f"{_DIM}\u00b7 init {fields}{_RESET}"
        return f"{_DIM}\u00b7 {subtype}{_RESET}"

    def _message(self, event: dict[str, object]) -> str | None:
        role = event.get("type")
        message = event.get("message")
        content = message.get("content") if isinstance(message, dict) else None
        if isinstance(content, str):
            return self._text(role, content)
        out: list[str] = []
        for block in content if isinstance(content, list) else []:
            if not isinstance(block, dict):
                continue
            rendered = self._block(role, block)
            if rendered is not None:
                out.append(rendered)
        return "\n".join(out) or None

    def _block(self, role: object, block: dict[str, object]) -> str | None:
        kind = block.get("type")
        if kind == "text":
            return self._text(role, str(block.get("text", "")))
        if kind == "thinking":
            first = str(block.get("thinking", "")).strip().splitlines()
            head = _clip(first[0], _THINKING_LIMIT) if first else ""
            return f"{_DIM}\u273b {head}{_RESET}"
        if kind in ("tool_use", "server_tool_use"):
            name = str(block.get("name", "tool"))
            block_id = block.get("id")
            if isinstance(block_id, str):
                self._tool_names[block_id] = name
            return f"{_CYAN}{_BOLD}\u23fa {name}{_RESET}{_CYAN} {_compact(block.get('input'), _INPUT_LIMIT)}{_RESET}"
        if kind == "tool_result":
            name = self._tool_names.get(str(block.get("tool_use_id")), "tool")
            color = _RED if block.get("is_error") else _DIM
            lines = _excerpt(_tool_result_text(block.get("content")))
            body = "\n".join(f"    {line}" for line in lines[1:])
            head = f"{color}  \u23bf {name}: {lines[0]}{_RESET}"
            return head if not body else f"{head}\n{color}{body}{_RESET}"
        return f"{_DIM}{_compact(block, _UNKNOWN_LIMIT)}{_RESET}"

    def _text(self, role: object, text: str) -> str:
        marker = "\u23fa" if role == "assistant" else "\u276f"
        lines = text.splitlines() or [""]
        head = f"{_BOLD}{marker} {lines[0]}{_RESET}" if role == "assistant" else f"{_DIM}{marker} {lines[0]}{_RESET}"
        rest = [f"  {line}" for line in lines[1:]]
        return "\n".join([head, *rest])

    def _result(self, event: dict[str, object]) -> str:
        failed = bool(event.get("is_error"))
        duration = event.get("duration_ms")
        parts = [] if not isinstance(duration, (int, float)) else [f"{duration / 1000:.1f}s"]
        turns = event.get("num_turns")
        if turns is not None:
            parts.append(f"{turns} turns")
        cost = event.get("total_cost_usd")
        if isinstance(cost, (int, float)):
            parts.append(f"${cost:.4f}")
        head = (
            f"{_RED}\u2717 {event.get('subtype') or 'error'}"
            if failed
            else f"{_GREEN}\u2714 done"
        )
        line = f"{head}{' \u00b7 ' if parts else ''}{' \u00b7 '.join(parts)}{_RESET}"
        result = event.get("result")
        if failed and result:
            line += f"\n{_RED}{_clip(str(result), _UNKNOWN_LIMIT)}{_RESET}"
        return line
