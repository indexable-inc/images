"""Default-on notification sources: noteworthy host events, pushed into the agent.

Each source is a small pluggable module (a :class:`Source` subclass) that polls
one cheap host interface and returns :class:`Event` rows. The runner delivers
every event onto the tier-3 mailbox outbox as a broadcast -- the exact rows the
stdio transport pump already turns into ``notifications/claude/channel`` events
(what ``pr_watch`` and the kernel's ``notify()`` produce) -- so a source event
wakes the connected agent session the same way a finished background job does,
with no new delivery mechanism.

The contract every source honors:

  - **Default-on where sane.** A source declares its platforms and default
    policy. ``IX_MCP_NOTIFY=0`` (or false/no/off) turns the whole framework
    off; ``IX_MCP_NOTIFY_<NAME>=0`` turns one source off. A source shipped
    default-off instead needs ``IX_MCP_NOTIFY_<NAME>=1`` to run.
  - **Cheap.** Sources poll at a low per-source cadence, never a busy loop.
    ``poll()`` is a plain sync method run on a worker thread
    (``asyncio.to_thread``), so blocking sqlite/subprocess/file IO is fine
    there and never stalls the server's event loop.
  - **Failure-isolated.** A source that cannot start skips with one stderr
    line. A running source that raises :class:`SourceUnavailable` -- or fails
    several polls in a row -- disables itself with one stderr line; the server
    and the other sources are untouched.
  - **Native-ready.** ``poll()`` returns plain data with no framework types
    beyond :class:`Event`, so a future native backend (e.g. a Rust/pyo3
    module watching an OS event API instead of polling) can implement a
    Source without the runner changing.

Shipped sources: :mod:`.imessage` (incoming iMessages; macOS, default on) and
:mod:`.system` (adverse system events -- low disk, memory pressure, thermal
throttling, OOM kills, high load; macOS + Linux, default on).

Channels are stdio-only (see :mod:`ix_notebook_mcp.transport`), so the
framework only starts for the stdio transport; anywhere else the rows would
just age out of the mailbox unread.
"""

from __future__ import annotations

import abc
import asyncio
import json
import os
import sys
from dataclasses import dataclass, field
from typing import ClassVar

from .. import mailbox
from ..config import Config

# A source is disabled after this many consecutive poll failures: transient
# hiccups (a locked database, a slow subprocess) retry silently, a persistently
# broken source gets one stderr line and stops instead of erroring forever.
_MAX_CONSECUTIVE_FAILURES = 5


@dataclass(frozen=True)
class Event:
    """One noteworthy occurrence, ready for the channel pump.

    ``content`` is the human/agent-readable line; ``meta`` becomes the channel
    tag's attributes (the runner adds ``source=<name>``). Keys must be
    identifiers (``[A-Za-z0-9_]``) -- Claude Code silently drops any other key
    (see ``runtime.notify``).
    """

    content: str
    meta: dict[str, str] = field(default_factory=dict)


class SourceUnavailable(Exception):
    """Raised by ``poll()`` when the source cannot keep running.

    The reason becomes the one stderr line; the source stops permanently (for
    this server process). For a condition that might clear, raise an ordinary
    exception instead -- the runner retries and only disables after
    ``_MAX_CONSECUTIVE_FAILURES`` in a row.
    """


class Source(abc.ABC):
    """One pluggable notification source.

    Subclasses set the class attributes and implement :meth:`poll`; the
    framework owns scheduling, delivery, env gating, and failure isolation.
    """

    # Identifier used in the env opt-out (IX_MCP_NOTIFY_<NAME>) and event meta.
    name: ClassVar[str]
    # sys.platform values this source runs on; None means every platform.
    platforms: ClassVar[tuple[str, ...] | None] = None
    # Whether the source runs with no env var set (the opt-out/opt-in switch).
    default_on: ClassVar[bool] = True
    # Seconds between polls.
    interval: ClassVar[float] = 60.0

    def available(self) -> str | None:
        """Why this source cannot start (one skip line), or None to run."""
        return None

    @abc.abstractmethod
    def poll(self) -> list[Event]:
        """One tick: the events since the previous call.

        Runs on a worker thread, so plain blocking IO is fine. The first call
        establishes any baseline -- a source must report what happens from
        startup on, never replay history.
        """


def registry() -> tuple[type[Source], ...]:
    """Every shipped source. Imported lazily so the package's framework types
    are fully defined before a source module (which imports them) loads."""
    from .imessage import IMessageSource
    from .system import SystemEventsSource

    return (IMessageSource, SystemEventsSource)


def framework_enabled() -> bool:
    """Whether notification sources run at all.

    Default ON -- events should improve the agent's context with zero config --
    so the env var is an opt-out only: ``IX_MCP_NOTIFY=0`` (or false/no/off)
    disables every source (mirrors ``config.mesh_enabled``).
    """
    return os.environ.get("IX_MCP_NOTIFY", "").strip().lower() not in ("0", "false", "no", "off")


def source_enabled(source: type[Source]) -> bool:
    """Whether one source runs, per ``IX_MCP_NOTIFY_<NAME>``.

    A default-on source is opt-out (only 0/false/no/off disable it); a
    default-off source is opt-in (only 1/true/yes/on enable it).
    """
    raw = os.environ.get(f"IX_MCP_NOTIFY_{source.name.upper()}", "").strip().lower()
    if source.default_on:
        return raw not in ("0", "false", "no", "off")
    return raw in ("1", "true", "yes", "on")


def _log(line: str) -> None:
    print(f"[ix-mcp] notifications: {line}", file=sys.stderr, flush=True)


def _deliver(source: Source, event: Event) -> None:
    """Queue one event on the in-process mailbox outbox, as a broadcast.

    Broadcast (``session=""``) on purpose: like an armed ``pr_watch``, a host
    event is news for whatever agent session is connected, not addressed to the
    session that started some job. The transport pump turns the row into a
    ``notifications/claude/channel`` event after the client's ``initialized``.
    """
    meta = {"source": source.name, **event.meta}
    mailbox.get_mailbox().add_outbox(content=event.content, meta=json.dumps(meta), session="")


async def _run_source(source: Source) -> None:
    """Poll one source forever, isolating its failures from everything else."""
    failures = 0
    while True:
        events: list[Event] = []
        try:
            events = await asyncio.to_thread(source.poll)
            failures = 0
        except SourceUnavailable as exc:
            _log(f"{source.name} disabled: {exc}")
            return
        except Exception as exc:
            failures += 1
            if failures >= _MAX_CONSECUTIVE_FAILURES:
                _log(f"{source.name} disabled after {failures} consecutive poll failures, last: {exc!r}")
                return
        for event in events:
            _deliver(source, event)
        await asyncio.sleep(source.interval)


async def _run_sources(sources: list[Source]) -> None:
    # gather (not TaskGroup): _run_source never raises except on cancellation,
    # and one source's lifecycle must not be able to cancel its siblings.
    await asyncio.gather(*(_run_source(source) for source in sources))


def start(cfg: Config) -> asyncio.Task[None] | None:
    """Start the enabled sources as one background task, or skip (``None``).

    Every skip path logs at most one stderr line and never raises: like the
    mesh endpoint, notifications are a nicety and must not be able to take the
    MCP down. A platform mismatch skips silently -- an iMessage source absent
    on Linux is not news.
    """
    if cfg.transport != "stdio":
        return None  # channels are stdio-only; nothing could deliver the rows
    if not framework_enabled():
        _log("disabled: IX_MCP_NOTIFY=0")
        return None
    active: list[Source] = []
    for cls in registry():
        if cls.platforms is not None and sys.platform not in cls.platforms:
            continue
        if not source_enabled(cls):
            _log(f"{cls.name} disabled: IX_MCP_NOTIFY_{cls.name.upper()} opts out")
            continue
        source = cls()
        reason = source.available()
        if reason is not None:
            _log(f"{cls.name} disabled: {reason}")
            continue
        active.append(source)
    if not active:
        return None
    _log("watching " + ", ".join(f"{source.name} (every {source.interval:g}s)" for source in active))
    return asyncio.create_task(_run_sources(active), name="ix-mcp-notifications")
