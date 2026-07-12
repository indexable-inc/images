"""Search local Claude Code history: one ranked row per matching session.

``await claude_history.search(pattern)`` greps every transcript under
``~/.claude/projects`` (ripgrep, via the bundled :mod:`fsearch`) and returns a
``polars.DataFrame`` with one row per matching *session*, ranked by hit count::

    df = await claude_history.search("golden snapshot")
    # session_id | cwd | hits | first_user_message | started_at | ended_at | ...

The per-session columns answer "which past conversation was that": the session
id, the un-munged project ``cwd``, first/last message timestamps, how many
transcript lines matched, and the first *real* user message -- harness-injected
meta records (``isMeta``, ``<...>``/``Caveat:`` prefixes), tool-result-only
entries, and pasted-TUI noise are all skipped (issue #2245).

The transcript schema is deliberately NOT re-implemented here: parsing reuses
:func:`distiller.transcripts.parse_session` (packages/agent/distiller), the
same reader the transcript distiller runs, so the schema knowledge stays owned
in one place on the Python side (the Rust owner is
``packages/search/source/claude``).
"""

from __future__ import annotations

import asyncio
import os
from datetime import UTC, datetime
from pathlib import Path

import polars as pl
from polars.datatypes import DataType, DataTypeClass
from distiller.transcripts import Session, parse_session, resolve_cwd

import fsearch

__all__ = ["DEFAULT_ROOT", "search"]

__version__ = "0.1.0"

DEFAULT_ROOT = "~/.claude/projects"

_SCHEMA: dict[str, DataTypeClass | DataType] = {
    "session_id": pl.Utf8,
    "cwd": pl.Utf8,
    "hits": pl.Int64,
    "first_user_message": pl.Utf8,
    "started_at": pl.Datetime(time_zone="UTC"),
    "ended_at": pl.Datetime(time_zone="UTC"),
    "messages": pl.Int64,
    "git_branch": pl.Utf8,
    "path": pl.Utf8,
}


def _when(epoch: float | None) -> datetime | None:
    """Epoch seconds (the distiller's timestamp shape) to an aware datetime."""
    return None if epoch is None else datetime.fromtimestamp(epoch, tz=UTC)


def _sessions(paths: list[str]) -> dict[str, Session]:
    """Parse each matching transcript once, keyed by path.

    A file that yields no session (marker/snapshot-only, or no real user
    message) is dropped: it has no session identity worth ranking.
    """
    out: dict[str, Session] = {}
    for path in paths:
        session = parse_session(Path(path))
        if session is not None:
            out[path] = session
    return out


async def search(
    pattern: str,
    root: str | os.PathLike[str] = DEFAULT_ROOT,
    *,
    ignore_case: bool = True,
    fixed: bool = False,
    limit: int = 100,
    max_matches: int = 100_000,
    timeout: float = 60.0,
    include_subagents: bool = False,
) -> pl.DataFrame:
    """One ranked row per session whose transcript matches ``pattern``.

    ``pattern`` is a regex (``fixed=True`` for a literal), case-insensitive by
    default. ``root`` is the Claude projects directory. ``limit`` caps the
    returned *sessions*; ``max_matches`` caps the underlying grep rows (the
    safety bound on a very broad pattern) and ``timeout`` bounds the scan.
    Subagent transcripts (``.../subagents/*.jsonl``) are folded out unless
    ``include_subagents=True`` -- the parent session carries the outcome.

    Columns: ``session_id, cwd, hits, first_user_message, started_at,
    ended_at, messages, git_branch, path``, sorted by ``hits`` descending.
    ``cwd`` is the session's recorded working directory (the un-munged project
    path), falling back to the path decoded from the transcript directory
    name. When the scan was cut short (timeout, or ``max_matches``) the frame
    comes back as :class:`fsearch.PartialFrame` with ``.truncated`` /
    ``.reason`` set, so a capped scan is never mistaken for a complete one.
    """
    matches = await fsearch.grep(
        pattern,
        root,
        ignore_case=ignore_case,
        fixed=fixed,
        glob="*.jsonl",
        hidden=True,
        no_ignore=True,
        limit=max_matches,
        timeout=timeout,
    )
    truncated = bool(getattr(matches, "truncated", False))
    reason = str(getattr(matches, "reason", "")) or "the underlying scan was cut short"
    if not include_subagents:
        matches = matches.filter(~pl.col("path").str.contains("/subagents/", literal=True))
    if matches.is_empty():
        empty = pl.DataFrame(schema=_SCHEMA)
        return fsearch.PartialFrame(empty, schema=_SCHEMA, reason=reason) if truncated else empty

    counts: dict[str, int] = {
        str(path): int(hits)
        for path, hits in matches.group_by("path").len(name="hits").iter_rows()
    }
    sessions = await asyncio.to_thread(_sessions, sorted(counts))
    rows: list[dict[str, object]] = [
        {
            "session_id": session.session_id,
            "cwd": resolve_cwd(session.cwd, Path(path).parent.name),
            "hits": counts[path],
            "first_user_message": session.goal,
            "started_at": _when(session.first_ts),
            "ended_at": _when(session.last_ts),
            "messages": session.message_count,
            "git_branch": session.git_branch,
            "path": path,
        }
        for path, session in sessions.items()
    ]
    frame = (
        pl.DataFrame(rows, schema=_SCHEMA)
        .sort("hits", "ended_at", descending=True, nulls_last=True)
        .head(limit)
    )
    return fsearch.PartialFrame(frame, schema=_SCHEMA, reason=reason) if truncated else frame
