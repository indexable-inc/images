"""The notification-source framework and its shipped sources.

Covers the framework contract (default-on env gating, broadcast delivery onto
the mailbox outbox, failure isolation between sources) plus the two shipped
sources against fakes: a temp SQLite database mimicking the ``chat.db`` schema
for iMessage, and injected probes for the system source. Hermetic: no real
Messages database, no subprocesses, no network.
"""

from __future__ import annotations

import asyncio
import json
import sqlite3
import sys
import threading
from pathlib import Path
from typing import ClassVar

import pytest

from ix_notebook_mcp import mailbox, notifications
from ix_notebook_mcp.config import Config
from ix_notebook_mcp.notifications import Event, Source, SourceUnavailable
from ix_notebook_mcp.notifications.imessage import IMessageSource
from ix_notebook_mcp.notifications.system import SystemEventsSource, _disk_probe


def _box() -> mailbox.Mailbox:
    box = mailbox.get_mailbox()
    box.reset()
    return box


# ---------------------------------------------------------------------------
# Env gating
# ---------------------------------------------------------------------------


def test_framework_enabled_default_on(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("IX_MCP_NOTIFY", raising=False)
    assert notifications.framework_enabled()
    for value in ("0", "false", "no", "off", " OFF "):
        monkeypatch.setenv("IX_MCP_NOTIFY", value)
        assert not notifications.framework_enabled(), f"IX_MCP_NOTIFY={value!r} must disable"
    monkeypatch.setenv("IX_MCP_NOTIFY", "1")
    assert notifications.framework_enabled()


def test_source_enabled_default_on_is_opt_out(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("IX_MCP_NOTIFY_IMESSAGE", raising=False)
    assert notifications.source_enabled(IMessageSource)
    monkeypatch.setenv("IX_MCP_NOTIFY_IMESSAGE", "0")
    assert not notifications.source_enabled(IMessageSource)


def test_source_enabled_default_off_is_opt_in(monkeypatch: pytest.MonkeyPatch) -> None:
    class Quiet(Source):
        name: ClassVar[str] = "quiet"
        default_on: ClassVar[bool] = False

        def poll(self) -> list[Event]:
            return []

    monkeypatch.delenv("IX_MCP_NOTIFY_QUIET", raising=False)
    assert not notifications.source_enabled(Quiet)
    monkeypatch.setenv("IX_MCP_NOTIFY_QUIET", "1")
    assert notifications.source_enabled(Quiet)
    monkeypatch.setenv("IX_MCP_NOTIFY_QUIET", "0")
    assert not notifications.source_enabled(Quiet)


# ---------------------------------------------------------------------------
# The runner: delivery and failure isolation
# ---------------------------------------------------------------------------


class _OneShot(Source):
    """Emits one event on its second poll (the first is the baseline)."""

    name: ClassVar[str] = "oneshot"
    interval: ClassVar[float] = 0.01

    def __init__(self) -> None:
        self.polls = 0

    def poll(self) -> list[Event]:
        self.polls += 1
        if self.polls == 2:
            return [Event(content="it happened", meta={"detail": "x"})]
        return []


class _Broken(Source):
    name: ClassVar[str] = "broken"
    interval: ClassVar[float] = 0.001

    def __init__(self) -> None:
        self.polls = 0
        self.capped = threading.Event()  # set once enough failures accrued to disable

    def poll(self) -> list[Event]:
        self.polls += 1
        if self.polls >= 5:
            self.capped.set()
        raise RuntimeError("boom")


class _Refusing(Source):
    name: ClassVar[str] = "refusing"
    interval: ClassVar[float] = 0.01

    def __init__(self) -> None:
        self.polls = 0

    def poll(self) -> list[Event]:
        self.polls += 1
        raise SourceUnavailable("cannot run here")


async def _drain_until(box: mailbox.Mailbox, count: int, timeout: float = 2.0) -> list[dict]:
    rows: list[dict] = []
    async with asyncio.timeout(timeout):
        while len(rows) < count:
            rows.extend(box.take_outbox())
            await asyncio.sleep(0.01)
    return rows


def test_runner_delivers_broadcast_rows_with_source_meta() -> None:
    async def run() -> None:
        box = _box()
        task = asyncio.create_task(notifications._run_sources([_OneShot()]))
        try:
            rows = await _drain_until(box, 1)
        finally:
            task.cancel()
        assert rows[0]["content"] == "it happened"
        assert rows[0]["session"] == ""  # broadcast: any connected session's pump delivers it
        assert json.loads(rows[0]["meta"]) == {"source": "oneshot", "detail": "x"}

    asyncio.run(run())


def test_runner_isolates_a_broken_source(capsys: pytest.CaptureFixture[str]) -> None:
    """A source failing every poll disables itself; its sibling keeps going."""

    async def run() -> None:
        box = _box()
        good = _OneShot()
        broken = _Broken()
        task = asyncio.create_task(notifications._run_sources([broken, good]))
        try:
            rows = await _drain_until(box, 1)
            # Let the failure cap trip, plus one tick for the disable line.
            assert await asyncio.to_thread(broken.capped.wait, 2.0)
            await asyncio.sleep(0.05)
        finally:
            task.cancel()
        assert rows[0]["content"] == "it happened"

    asyncio.run(run())
    assert "broken disabled after 5 consecutive poll failures" in capsys.readouterr().err


def test_runner_disables_unavailable_source_after_one_line(
    capsys: pytest.CaptureFixture[str],
) -> None:
    async def run() -> None:
        _box()
        source = _Refusing()
        await notifications._run_source(source)  # returns instead of looping
        assert source.polls == 1

    asyncio.run(run())
    err = capsys.readouterr().err
    assert err.count("refusing disabled: cannot run here") == 1


# ---------------------------------------------------------------------------
# start(): skip paths
# ---------------------------------------------------------------------------


def test_start_skips_non_stdio_transports(tmp_path: Path) -> None:
    async def run() -> None:
        assert notifications.start(Config(workdir=tmp_path, transport="http")) is None
        assert notifications.start(Config(workdir=tmp_path, transport="none")) is None

    asyncio.run(run())


def test_start_skips_when_framework_disabled(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("IX_MCP_NOTIFY", "0")

    async def run() -> None:
        assert notifications.start(Config(workdir=tmp_path, transport="stdio")) is None

    asyncio.run(run())


def test_start_applies_platform_env_and_availability_gates(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Platform mismatches skip silently; env opt-outs and unavailable sources
    skip with one line; what remains runs."""

    class Elsewhere(Source):
        name: ClassVar[str] = "elsewhere"
        platforms: ClassVar[tuple[str, ...] | None] = ("not-this-platform",)

        def poll(self) -> list[Event]:
            return []

    class OptedOut(Source):
        name: ClassVar[str] = "optedout"

        def poll(self) -> list[Event]:
            return []

    class NotReady(Source):
        name: ClassVar[str] = "notready"

        def available(self) -> str | None:
            return "missing its database"

        def poll(self) -> list[Event]:
            return []

    monkeypatch.delenv("IX_MCP_NOTIFY", raising=False)
    monkeypatch.setenv("IX_MCP_NOTIFY_OPTEDOUT", "0")
    monkeypatch.setattr(
        notifications, "registry", lambda: (Elsewhere, OptedOut, NotReady, _OneShot)
    )

    async def run() -> None:
        task = notifications.start(Config(workdir=tmp_path, transport="stdio"))
        assert task is not None
        task.cancel()

    asyncio.run(run())


def test_default_registry_lists_shipped_sources() -> None:
    assert [cls.name for cls in notifications.registry()] == ["imessage", "system"]
    assert all(issubclass(cls, Source) for cls in notifications.registry())


# ---------------------------------------------------------------------------
# The iMessage source, against a temp database mimicking chat.db
# ---------------------------------------------------------------------------


def _make_chat_db(path: Path) -> sqlite3.Connection:
    con = sqlite3.connect(path)
    con.executescript(
        """
        CREATE TABLE handle (ROWID INTEGER PRIMARY KEY, id TEXT);
        CREATE TABLE message (
            ROWID INTEGER PRIMARY KEY,
            date INTEGER,
            text TEXT,
            attributedBody BLOB,
            handle_id INTEGER,
            is_from_me INTEGER
        );
        """
    )
    con.commit()
    return con


def _insert(
    con: sqlite3.Connection,
    *,
    text: str | None,
    handle_id: int | None = 1,
    from_me: int = 0,
    date: int = 700_000_000_000_000_000,
) -> None:
    con.execute(
        "INSERT INTO message (date, text, attributedBody, handle_id, is_from_me)"
        " VALUES (?, ?, NULL, ?, ?)",
        (date, text, handle_id, from_me),
    )
    con.commit()


def test_imessage_baselines_then_reports_only_new_incoming(tmp_path: Path) -> None:
    db = tmp_path / "chat.db"
    con = _make_chat_db(db)
    con.execute("INSERT INTO handle (ROWID, id) VALUES (1, '+12025550123')")
    _insert(con, text="ancient history")

    source = IMessageSource(db=db)
    assert source.available() is None
    assert source.poll() == []  # baseline: pre-existing rows never replay

    _insert(con, text="hey, are you around?")
    _insert(con, text="my own reply", from_me=1)
    events = source.poll()
    assert len(events) == 1
    assert events[0].content == "iMessage from +12025550123: hey, are you around?"
    assert events[0].meta["handle"] == "+12025550123"
    assert events[0].meta["at"].startswith("2023-03")  # Apple ns epoch converted to UTC
    assert source.poll() == []  # nothing is ever reported twice
    con.close()


def test_imessage_placeholder_for_undecodable_body_and_unknown_handle(tmp_path: Path) -> None:
    db = tmp_path / "chat.db"
    con = _make_chat_db(db)
    source = IMessageSource(db=db)
    source.poll()
    _insert(con, text=None, handle_id=None, date=0)
    events = source.poll()
    assert events[0].content == (
        "iMessage from unknown sender: [no text: an attachment, tapback, or rich content]"
    )
    assert "at" not in events[0].meta
    con.close()


def test_imessage_caps_a_burst_but_never_replays_it(tmp_path: Path) -> None:
    from ix_notebook_mcp.notifications import imessage as imessage_mod

    db = tmp_path / "chat.db"
    con = _make_chat_db(db)
    source = IMessageSource(db=db)
    source.poll()
    for n in range(imessage_mod._MAX_EVENTS_PER_POLL + 10):
        _insert(con, text=f"msg {n}")
    assert len(source.poll()) == imessage_mod._MAX_EVENTS_PER_POLL
    assert source.poll() == []  # the cursor jumped past the capped tail: a gap, not a flood
    con.close()


def test_imessage_missing_database_is_an_availability_skip(tmp_path: Path) -> None:
    source = IMessageSource(db=tmp_path / "nope" / "chat.db")
    reason = source.available()
    assert reason is not None
    assert "no Messages database" in reason


def test_imessage_unopenable_database_disables_with_full_disk_access_hint(
    tmp_path: Path,
) -> None:
    # A directory at the path makes sqlite's open fail the same way a
    # permission denial does, exercising the disable-with-remedy path.
    denied = tmp_path / "chat.db"
    denied.mkdir()
    source = IMessageSource(db=denied)
    with pytest.raises(SourceUnavailable, match="Full Disk Access"):
        source.poll()


def test_imessage_gated_to_darwin_and_default_on() -> None:
    assert IMessageSource.platforms == ("darwin",)
    assert IMessageSource.default_on
    if sys.platform != "darwin":  # the registry gate keeps it off elsewhere
        assert sys.platform not in IMessageSource.platforms


# ---------------------------------------------------------------------------
# The system source, against injected probes
# ---------------------------------------------------------------------------


def test_system_source_edge_triggers_per_condition() -> None:
    conditions: dict[str, Event] = {}
    source = SystemEventsSource(probes=(lambda: dict(conditions),))

    assert source.poll() == []
    conditions["disk:/"] = Event(content="low disk space on /", meta={"kind": "disk"})
    assert [e.content for e in source.poll()] == ["low disk space on /"]
    assert source.poll() == []  # still low: one event at onset, not one per tick
    conditions.clear()
    assert source.poll() == []  # cleared: re-arms silently
    conditions["disk:/"] = Event(content="low disk space on /", meta={"kind": "disk"})
    assert len(source.poll()) == 1  # re-crossing the threshold fires again


def test_system_source_disables_one_broken_probe_and_keeps_the_rest(
    capsys: pytest.CaptureFixture[str],
) -> None:
    def broken() -> dict[str, Event]:
        raise RuntimeError("bad probe")

    healthy_calls: list[int] = []

    def healthy() -> dict[str, Event]:
        healthy_calls.append(1)
        return {}

    source = SystemEventsSource(probes=(broken, healthy))
    for _ in range(5):
        assert source.poll() == []
    assert len(healthy_calls) == 5
    err = capsys.readouterr().err
    assert err.count("system probe broken disabled") == 1


def test_system_source_gives_up_when_every_probe_is_dead() -> None:
    def broken() -> dict[str, Event]:
        raise RuntimeError("bad probe")

    source = SystemEventsSource(probes=(broken,))
    for _ in range(2):
        source.poll()
    with pytest.raises(SourceUnavailable, match="every system probe"):
        source.poll()


def test_disk_probe_flags_a_nearly_full_filesystem(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class _Vfs:
        f_bavail = 10  # 10 * 4096 bytes free: far under both floors
        f_frsize = 4096
        f_blocks = 1_000_000

    monkeypatch.setattr("os.statvfs", lambda path: _Vfs())
    found = _disk_probe(paths=(tmp_path,))
    assert list(found) == [f"disk:{tmp_path}"]
    event = found[f"disk:{tmp_path}"]
    assert "low disk space" in event.content
    assert event.meta == {"kind": "disk", "path": str(tmp_path)}


def test_disk_probe_dedupes_paths_on_one_device(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class _Vfs:
        f_bavail = 10
        f_frsize = 4096
        f_blocks = 1_000_000

    monkeypatch.setattr("os.statvfs", lambda path: _Vfs())
    nested = tmp_path / "nested"
    nested.mkdir()
    assert len(_disk_probe(paths=(tmp_path, nested))) == 1


def test_system_source_platforms_and_default() -> None:
    assert SystemEventsSource.platforms == ("darwin", "linux")
    assert SystemEventsSource.default_on
    assert SystemEventsSource(probes=()).available() == "no probes for this platform"
