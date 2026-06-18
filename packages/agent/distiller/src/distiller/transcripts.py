"""Read Claude Code transcripts and extract distillation signals.

One ``Session`` per top-level ``*.jsonl`` under ``~/.claude/projects/<dir>/``
(subagent transcripts under ``.../subagents/`` are folded into nothing -- they
are skipped; the parent session carries the outcome). Every field is treated
as optional and unparseable lines are skipped, mirroring the Rust adapter
(``packages/search/source/claude``): the transcript schema is external and evolves.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from datetime import datetime, timezone, UTC
from pathlib import Path

# User messages matching these near the start signal a correction of the
# assistant's previous action -- the highest-value failure signal we have
# (ReasoningBank distills guardrails from failures, not only wins).
_CORRECTION_RE = re.compile(
    r"^\s*(no\b|nope\b|not\b|wrong\b|stop\b|wait\b|don'?t\b|undo\b|revert\b"
    r"|that'?s (not|wrong)|incorrect\b|actually\b|instead\b|why did you\b"
    r"|you (should not|shouldn'?t)\b)",
    re.IGNORECASE,
)

# Cheap success markers in the final stretch of a session. "Pushed to main"
# is the wrapper-mandated landing banner (see research-prior-art-memory.md).
_SUCCESS_RE = re.compile(
    r"(pushed to main|\U0001f680|all tests pass|tests? pass(ed)?\b|merged\b"
    r"|landed on main|build succeeded)",
    re.IGNORECASE,
)


@dataclass
class Session:
    """Signals extracted from one transcript file."""

    session_id: str
    path: str
    cwd: str | None = None
    git_branch: str | None = None
    first_ts: float | None = None
    last_ts: float | None = None
    goal: str | None = None
    corrections: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    final_assistant: str | None = None
    success_markers: list[str] = field(default_factory=list)
    models: list[str] = field(default_factory=list)
    message_count: int = 0
    outcome: str = "unknown"  # success | failure | mixed | unknown

    def fingerprint(self) -> str:
        """Cheap change marker so a still-growing session re-distills."""
        return f"{self.message_count}:{self.last_ts or 0}"


def _parse_ts(value: object) -> float | None:
    if not isinstance(value, str):
        return None
    try:
        return datetime.fromisoformat(value).timestamp()
    except ValueError:
        return None


def _clip(text: str, limit: int) -> str:
    text = " ".join(text.split())
    if len(text) <= limit:
        return text
    return text[: limit - 1] + "…"


def _text_blocks(content: object) -> str:
    """Plain text of a message content (string or block list)."""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                text = block.get("text")
                if isinstance(text, str):
                    parts.append(text)
        return "\n".join(parts)
    return ""


def _is_meta_user_text(text: str) -> bool:
    """Harness-injected user content, not a human signal."""
    stripped = text.lstrip()
    return stripped.startswith(("<", "Caveat:"))


def parse_session(path: Path) -> Session | None:
    """Extract a :class:`Session` from one transcript file.

    Returns ``None`` when the file carries no user/assistant messages (marker
    or snapshot-only files).
    """

    session = Session(session_id=path.stem, path=str(path))
    saw_user = False
    last_error_ts: float | None = None
    try:
        handle = path.open("r", encoding="utf-8", errors="replace")
    except OSError:
        return None
    with handle:
        for raw_line in handle:
            line = raw_line.strip()
            if not line:
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue  # truncated mid-write line; same policy as the Rust adapter
            if not isinstance(record, dict):
                continue
            rtype = record.get("type")
            if rtype not in ("user", "assistant"):
                continue
            if record.get("isSidechain"):
                continue
            message = record.get("message")
            if not isinstance(message, dict):
                continue
            ts = _parse_ts(record.get("timestamp"))
            if ts is not None:
                session.first_ts = ts if session.first_ts is None else min(session.first_ts, ts)
                session.last_ts = ts if session.last_ts is None else max(session.last_ts, ts)
            if session.cwd is None and isinstance(record.get("cwd"), str):
                session.cwd = record["cwd"]
            if session.git_branch is None and isinstance(record.get("gitBranch"), str):
                session.git_branch = record["gitBranch"]
            sid = record.get("sessionId")
            if isinstance(sid, str) and sid:
                session.session_id = sid
            session.message_count += 1

            content = message.get("content")
            if rtype == "user":
                # tool_result blocks ride user-role lines; mine them for errors.
                if isinstance(content, list):
                    for block in content:
                        if not isinstance(block, dict):
                            continue
                        if block.get("type") == "tool_result" and block.get("is_error"):
                            text = _text_blocks(block.get("content")) or str(
                                block.get("content", "")
                            )
                            if text and len(session.errors) < 8:
                                session.errors.append(_clip(text, 280))
                            last_error_ts = ts
                    continue
                if not isinstance(content, str) or not content.strip():
                    continue
                if _is_meta_user_text(content):
                    continue
                if not saw_user:
                    saw_user = True
                    session.goal = _clip(content, 600)
                elif _CORRECTION_RE.search(content) and len(session.corrections) < 6:
                    session.corrections.append(_clip(content, 320))
            else:  # assistant
                model = message.get("model")
                if isinstance(model, str) and model and model not in session.models:
                    session.models.append(model)
                text = _text_blocks(content)
                if text.strip():
                    session.final_assistant = _clip(text, 700)
                    marker = _SUCCESS_RE.search(text)
                    if marker and len(session.success_markers) < 4:
                        session.success_markers.append(marker.group(0))

    if session.message_count == 0 or not saw_user:
        return None

    # Outcome label: noisy-but-sufficient heuristics (an LLM judge refines
    # this inside the distillation prompt; ReasoningBank shows label noise is
    # tolerable).
    has_success = bool(session.success_markers)
    error_at_end = (
        last_error_ts is not None
        and session.last_ts is not None
        and session.last_ts - last_error_ts < 120
    )
    if has_success and not error_at_end:
        session.outcome = "success"
    elif error_at_end or (session.errors and not has_success and session.corrections):
        session.outcome = "failure"
    elif session.errors or session.corrections:
        session.outcome = "mixed"
    return session


def project_slug(project: str) -> str:
    slug = re.sub(r"[^A-Za-z0-9._-]+", "-", project.strip("/")).strip("-")
    return slug or "unknown"


# cwd values that are not a project checkout: temp scratch and the distiller's
# own ``claude -p`` sandboxes (``/tmp/ix-distiller-*``), plus bare ``$HOME`` /
# ``/root``. Keying lessons on these creates silos that help no future session.
_SCRATCH_RE = re.compile(r"(^|/)(tmp|private/tmp|var/folders)(/|$)|ix-distiller-")
_HOME_RE = re.compile(r"^/(home|Users)/[^/]+/?$|^/root/?$")
# Generic container dirs that hold many repos: a cwd ending here names no single
# repo, so it is not a partition of its own.
_CONTAINER_DIRS = frozenset(
    {
        "github", "projects", "src", "code", "repos", "repositories",
        "dev", "git", "work", "documents", "desktop", "downloads",
    }
)


def _resolve_path(cwd: str | None, transcript_dir: str) -> str:
    """The session's working dir: the recorded cwd, else the decoded dir name."""
    if cwd:
        return cwd
    # `-home-andrew-index` -> `/home/andrew/index` (lossy but stable).
    return "/" + transcript_dir.strip("-").replace("-", "/")


def repo_identity(cwd: str | None, transcript_dir: str) -> str | None:
    """Canonical repo slug for a session, or ``None`` for non-repo cwds.

    The memory partition key must be repo identity, not the raw cwd: two clones
    of one repo (``~/Github/nox`` and ``~/nox``) must collapse to one silo, and
    scratch dirs and bare ``$HOME`` must not become silos at all. Transcripts
    carry no git remote (only ``cwd`` and ``gitBranch``), so this is a path
    heuristic: drop a worktree suffix, reject scratch/home/container dirs, then
    take the project-root basename. It cannot collapse two clones whose
    basenames differ, but it fixes the common clone and worktree cases.
    """
    path = _resolve_path(cwd, transcript_dir).rstrip("/")
    if not path or _SCRATCH_RE.search(path) or _HOME_RE.match(path):
        return None
    # A git worktree lives at ``<repo>/.claude/worktrees/<name>``; the repo root
    # is the segment before ``.claude``, not the worktree name.
    marker = "/.claude/"
    if marker in path:
        path = path[: path.index(marker)]
    name = path.rsplit("/", 1)[-1]
    if name.startswith(".") or name.lower() in _CONTAINER_DIRS:
        return None
    slug = project_slug(name)
    return slug if slug != "unknown" else None


def legacy_state_slugs(sessions: list[Session]) -> list[str]:
    """Old per-cwd state filenames the given sessions wrote before repo-keying.

    Before this change the state file was keyed on the raw cwd slug
    (``/home/u/repo`` -> ``home-u-repo.json``); now it is keyed on the repo
    identity (``repo.json``). On the first run after the switch the new path is
    absent, so ``state.load`` would silently start fresh and the rewritten
    corpus would drop every previously learned item. Returning the legacy
    slug(s) for a repo's sessions lets ``load`` migrate the old file forward
    (newest cwd first, so the most recent silo wins on collision).

    The old key used ``_resolve_path`` -- the recorded ``cwd`` when present, else
    the decoded transcript-dir name -- so a session that never recorded a ``cwd``
    still wrote a legacy file under that decoded path. Use the same fallback here
    so those transcripts migrate too instead of starting from empty state.
    """
    seen: set[str] = set()
    slugs: list[str] = []
    for session in sorted(sessions, key=lambda s: s.last_ts or 0, reverse=True):
        raw = _resolve_path(session.cwd, Path(session.path).parent.name)
        slug = project_slug(raw)
        if slug == "unknown" or slug in seen:
            continue
        seen.add(slug)
        slugs.append(slug)
    return slugs


def scan(root: Path, days: float, now: float | None = None) -> dict[str, list[Session]]:
    """Parse every transcript under ``root`` newer than the window.

    Returns sessions grouped by repo slug, newest first. Sessions whose cwd is
    not a repo checkout (scratch dirs, bare ``$HOME``) are dropped. The mtime
    gate avoids parsing the long tail of old transcripts; the message-timestamp
    gate then trims sessions whose activity falls outside the window.
    """

    now = now if now is not None else datetime.now(UTC).timestamp()
    cutoff = now - days * 86400
    groups: dict[str, list[Session]] = {}
    if not root.is_dir():
        return groups
    for path in sorted(root.glob("*/*.jsonl")):
        if "subagents" in path.parts:
            continue
        try:
            if path.stat().st_mtime < cutoff:
                continue
        except OSError:
            continue
        session = parse_session(path)
        if session is None:
            continue
        if session.last_ts is not None and session.last_ts < cutoff:
            continue
        repo = repo_identity(session.cwd, path.parent.name)
        if repo is None:
            continue  # scratch / home / container dir -> not a repo silo
        groups.setdefault(repo, []).append(session)
    for sessions in groups.values():
        sessions.sort(key=lambda s: s.last_ts or 0, reverse=True)
    return groups


def digest(session: Session) -> str:
    """Compact per-session evidence block fed to the distiller model."""
    lines = [
        f"### session {session.session_id}",
        f"outcome-guess: {session.outcome}; messages: {session.message_count};"
        f" branch: {session.git_branch or '?'}",
    ]
    if session.goal:
        lines.append(f"goal: {session.goal}")
    lines.extend(f"user-correction: {correction}" for correction in session.corrections)
    lines.extend(f"tool-error: {error}" for error in session.errors[:4])
    if session.success_markers:
        lines.append("success-markers: " + ", ".join(session.success_markers))
    if session.final_assistant:
        lines.append(f"final-assistant: {session.final_assistant}")
    return "\n".join(lines)
