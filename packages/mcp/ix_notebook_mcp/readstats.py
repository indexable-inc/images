"""Per-session tracking of redundant file reads.

Every kernel path that reads a real file *into the agent's context* (the ``read``
MCP tool's file branch, ``view.cat``/``head``/``tail``, and any kernel read
helper) records the read here. A read is REDUNDANT when the same
``(absolute path, content bytes)`` pair was already read earlier in the same
kernel session: with perfect memory the agent would not have needed it again.
The KPI (indexable-inc/ix#6440) is ``redundant / total < 1%``.

The counters are CUMULATIVE per session (never per-window deltas): each emitted
``mcp_read_stats`` line reports the running totals since the session started, and
the ix fleet pipeline (indexable-inc/ix#6453) differences them itself. ``window_s``
is the fixed emit cadence, not a measurement window.

Memory is bounded by construction: only the 16-byte content digests are kept, never
the file bytes. The digest is over ``(absolute path, content)`` so the same bytes at
two paths count as two distinct reads (a genuine second read), while the same path
re-read unchanged is redundant.
"""

from __future__ import annotations

import dataclasses
import hashlib
import os
import pathlib
import sys

# The emit cadence in seconds, and the exact ``window_s`` value in every
# ``mcp_read_stats`` line. The ix fleet pipeline keys on this literal.
EMIT_WINDOW_S = 300

# The session key used when a read has no MCP session id (the shared namespace,
# e.g. reads driven through `/api/exec` in the daemon `notebook` shape). A stable
# string keeps the emitted `session` field non-empty and groupable.
_SHARED_SESSION = "shared"


def _digest(path: pathlib.Path, content: str | bytes) -> bytes:
    """A 16-byte blake2b digest of ``(absolute path, content)``.

    ``content`` is whatever the read already produced in memory -- the decoded
    text for the text-read paths, raw bytes for a bytes read -- so hashing never
    triggers a second disk read. Binding the path in means the same bytes read
    from two files are two reads, not a false redundancy; the path is absolutized
    first so ``foo.py`` and ``./foo.py`` collapse to one identity.
    """
    h = hashlib.blake2b(digest_size=16)
    h.update(os.fspath(path.resolve()).encode("utf-8"))
    h.update(b"\x00")
    h.update(content if isinstance(content, bytes) else content.encode("utf-8", "surrogatepass"))
    return h.digest()


@dataclasses.dataclass
class _SessionStats:
    """The cumulative read counters and seen-digest set for one session."""

    total_reads: int = 0
    redundant_reads: int = 0
    seen: set[bytes] = dataclasses.field(default_factory=set)
    # The counts as of the last emitted line, so the periodic emitter only speaks
    # when something changed (the issue's "IF counts changed").
    emitted_total: int = -1
    emitted_redundant: int = -1


class ReadStatsTracker:
    """Per-session redundant-read counters for one kernel process.

    One instance lives for the kernel's lifetime and holds a small map of session
    id -> :class:`_SessionStats`. It is driven entirely on the kernel's single
    event loop, so it needs no lock.
    """

    def __init__(self) -> None:
        self._by_session: dict[str, _SessionStats] = {}

    def _stats(self, session: str | None) -> _SessionStats:
        key = session or _SHARED_SESSION
        stats = self._by_session.get(key)
        if stats is None:
            stats = _SessionStats()
            self._by_session[key] = stats
        return stats

    def record(self, session: str | None, path: pathlib.Path, content: str | bytes) -> bool:
        """Record one file read; return whether it was redundant.

        ``content`` is the text (or bytes) already in memory from the read that
        just happened -- this never touches the disk again. A caller that could
        not read the file has no ``content`` to pass and never reaches here: an
        unreadable path is that read's own error path, not a swallow.
        """
        stats = self._stats(session)
        digest = _digest(path, content)
        redundant = digest in stats.seen
        stats.total_reads += 1
        if redundant:
            stats.redundant_reads += 1
        else:
            stats.seen.add(digest)
        return redundant

    def snapshot(self, session: str | None) -> dict[str, int]:
        """The live cumulative counters for ``session`` (for the agent to read its
        own redundancy rate). A session with no reads yet reports zeros."""
        stats = self._by_session.get(session or _SHARED_SESSION)
        if stats is None:
            return {"total_reads": 0, "redundant_reads": 0}
        return {"total_reads": stats.total_reads, "redundant_reads": stats.redundant_reads}

    def _line(self, session_key: str, stats: _SessionStats) -> str:
        """The exact one-line JSON contract parsed by the ix fleet pipeline.

        Built by hand (not ``json.dumps``) so the field order and spacing match
        the frozen contract in indexable-inc/ix#6453 byte-for-byte.
        """
        return (
            f'{{"event":"mcp_read_stats","session":"{session_key}",'
            f'"total_reads":{stats.total_reads},"redundant_reads":{stats.redundant_reads},'
            f'"window_s":{EMIT_WINDOW_S}}}'
        )

    def _emit(self, session_key: str, stats: _SessionStats) -> None:
        """Write one stats line to the kernel's stderr (which reaches journald in
        the deployed service) and mark the counts as emitted. Stderr, not stdout:
        in the stdio transport the real stdout carries the JSON-RPC stream, so a
        stray write there would corrupt the protocol; stderr always reaches the
        journal in both the stdio and daemon `notebook` shapes."""
        print(self._line(session_key, stats), file=sys.__stderr__, flush=True)
        stats.emitted_total = stats.total_reads
        stats.emitted_redundant = stats.redundant_reads

    def emit_changed(self) -> None:
        """Emit one line per session whose counts changed since its last emit.

        Called on the periodic (every ``EMIT_WINDOW_S``) tick.
        """
        for key, stats in self._by_session.items():
            if stats.total_reads != stats.emitted_total or stats.redundant_reads != stats.emitted_redundant:
                self._emit(key, stats)

    def emit_final(self) -> None:
        """Emit every session's final counts on clean shutdown, whether or not
        they changed since the last periodic emit, so the last window is never
        lost."""
        for key, stats in self._by_session.items():
            if stats.total_reads:
                self._emit(key, stats)


# The one tracker for this kernel process. Module-level so every read path shares
# it and the periodic emitter and shutdown hook reach the same counters.
_tracker = ReadStatsTracker()


def tracker() -> ReadStatsTracker:
    """The kernel's single :class:`ReadStatsTracker`."""
    return _tracker
