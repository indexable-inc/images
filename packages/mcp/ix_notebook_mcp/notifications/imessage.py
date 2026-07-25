"""Incoming iMessages as agent notifications (macOS, default on).

On macOS every conversation lives in the Messages SQLite database
(``~/Library/Messages/chat.db``). This source polls it read-only for new
``message`` rows joined with ``handle``, filtered to ``is_from_me = 0``, so
every incoming iMessage/SMS surfaces as a channel event within a few seconds
of arriving -- by default, with no opt-in. ``IX_MCP_NOTIFY_IMESSAGE=0`` turns
it off (and ``IX_MCP_NOTIFY=0`` turns off every source).

The baseline is the newest ROWID at startup: the source reports what arrives
from now on and never replays history. Reading ``chat.db`` requires the host
process to have **Full Disk Access** (System Settings > Privacy & Security >
Full Disk Access); without it the first poll's open fails and the source
disables itself with one stderr line saying exactly that, instead of erroring
forever or taking the server down.

Read-only access mirrors the bundled ``imessage`` kernel module
(``src/imessage``): ``mode=ro`` (not ``immutable=1``), because the database
runs in WAL mode where ``immutable`` would miss just-written rows, while a
plain read-only connection reads the WAL and coexists with the Messages app's
writer. Likewise the text decode: modern macOS often leaves ``message.text``
NULL and stores the body in ``message.attributedBody`` as an archived
``NSAttributedString``, which Foundation's ``NSUnarchiver`` decodes. (The
kernel module is bundled into the pinned interpreter, not a dependency of
this package, so the two cannot share code directly.)
"""

from __future__ import annotations

import sqlite3
from datetime import UTC, datetime
from pathlib import Path
from typing import ClassVar

from . import Event, Source, SourceUnavailable

CHAT_DB = Path("~/Library/Messages/chat.db")

# Core Data / Apple epoch: message.date is nanoseconds since 2001-01-01 UTC on
# modern macOS (it was seconds before High Sierra; magnitude tells them apart).
_APPLE_EPOCH_NS = int(datetime(2001, 1, 1, tzinfo=UTC).timestamp() * 1_000_000_000)

# One event per message, but bounded: a poll never emits more than this many
# rows, and the cursor still jumps to the database's newest ROWID, so a freak
# backlog degrades to a gap instead of flooding the agent's context.
_MAX_EVENTS_PER_POLL = 25
_MAX_TEXT_CHARS = 400


def _connect(path: Path) -> sqlite3.Connection:
    """Open chat.db read-only, converting a denied open into a clear disable.

    An open failure here is almost always a missing Full Disk Access grant, so
    the reason says so; SourceUnavailable makes the framework log it once and
    stop this source rather than retry a permission that will not change.
    """
    try:
        return sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    except sqlite3.Error as exc:
        raise SourceUnavailable(
            f"cannot open {path} ({exc}); reading the Messages database "
            "requires Full Disk Access -- grant it to the process running "
            "ix-mcp under System Settings > Privacy & Security > Full Disk "
            "Access, then restart"
        ) from exc


def _decode_attributed_body(blob: bytes | None) -> str | None:
    """Best-effort plain text from an archived ``NSAttributedString`` blob.

    Foundation (pyobjc) is present on the macOS interpreter; anywhere it is
    missing, or the blob does not decode, the caller falls back to a
    placeholder rather than dropping the notification.
    """
    if not blob:
        return None
    try:
        import Foundation
    except ImportError:
        return None
    try:
        data = Foundation.NSData.dataWithBytes_length_(blob, len(blob))
        obj = Foundation.NSUnarchiver.unarchiveObjectWithData_(data)
    except Exception:
        return None
    if obj is None:
        return None
    return str(obj.string() if hasattr(obj, "string") else obj)


def _to_iso(raw: int | None) -> str:
    """An Apple ``date`` value as an ISO UTC timestamp, or '' when absent."""
    if not raw:
        return ""
    ns = raw * 1_000_000_000 if abs(raw) < 100_000_000_000 else raw
    return datetime.fromtimestamp((ns + _APPLE_EPOCH_NS) / 1e9, tz=UTC).isoformat()


class IMessageSource(Source):
    """Every incoming iMessage/SMS, as it lands in ``chat.db``."""

    name: ClassVar[str] = "imessage"
    platforms: ClassVar[tuple[str, ...] | None] = ("darwin",)
    interval: ClassVar[float] = 5.0

    def __init__(self, db: Path | None = None) -> None:
        self._db = (db or CHAT_DB).expanduser()
        self._last_rowid: int | None = None

    def available(self) -> str | None:
        if not self._db.exists():
            return f"no Messages database at {self._db}"
        return None

    def poll(self) -> list[Event]:
        # A fresh connection per poll (the cadence makes it free) so a rotated
        # or repaired database is picked up and no handle is held across ticks.
        con = _connect(self._db)
        try:
            newest = self._max_rowid(con)
            if self._last_rowid is None:
                # Baseline: notify from now on, never replay history.
                self._last_rowid = newest
                return []
            rows = con.execute(
                "SELECT m.ROWID, m.date, m.text, m.attributedBody, h.id"
                " FROM message m LEFT JOIN handle h ON m.handle_id = h.ROWID"
                " WHERE m.ROWID > ? AND m.is_from_me = 0"
                " ORDER BY m.ROWID LIMIT ?",
                (self._last_rowid, _MAX_EVENTS_PER_POLL),
            ).fetchall()
            # Advance past everything -- rows beyond the cap and our own sent
            # messages included -- so nothing is ever reported twice.
            self._last_rowid = max(self._last_rowid, newest)
        finally:
            con.close()
        return [self._event(*row) for row in rows]

    @staticmethod
    def _max_rowid(con: sqlite3.Connection) -> int:
        return int(con.execute("SELECT COALESCE(MAX(ROWID), 0) FROM message").fetchone()[0])

    @staticmethod
    def _event(
        rowid: int,
        date: int | None,
        text: str | None,
        blob: bytes | None,
        handle: str | None,
    ) -> Event:
        body = (text if text is not None else _decode_attributed_body(blob)) or ""
        body = body.strip() or "[no text: an attachment, tapback, or rich content]"
        if len(body) > _MAX_TEXT_CHARS:
            body = body[:_MAX_TEXT_CHARS] + "..."
        sender = handle or "unknown sender"
        meta = {"handle": sender, "rowid": str(rowid)}
        when = _to_iso(date)
        if when:
            meta["at"] = when
        return Event(content=f"iMessage from {sender}: {body}", meta=meta)
