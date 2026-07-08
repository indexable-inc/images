"""The store's async facade: every call runs on one private worker thread, off
the caller's event loop, and the cheap `data_version` change gate the pane
bridge polls with (index#2348: inline blob reads on the shared loop starved MCP
stdio and presented as a kernel wedge)."""

from __future__ import annotations

import asyncio
import sqlite3
import threading
from pathlib import Path

import pytest

from ix_notebook_mcp import store
from ix_notebook_mcp.pane_bridge import _data_version


def test_async_conn_runs_off_loop_on_one_thread(tmp_path: Path) -> None:
    async def run() -> None:
        db = store.AsyncConn(tmp_path / "a.db")
        seen: set[int] = set()

        def probe(conn: sqlite3.Connection, value: int) -> int:
            seen.add(threading.get_ident())
            # The connection is live and usable where the call runs.
            assert conn.execute("SELECT ?", (value,)).fetchone()[0] == value
            return value

        try:
            assert [await db.run(probe, n) for n in range(3)] == [0, 1, 2]
        finally:
            await db.close()
        # Confined: every call landed on the same worker thread, never the loop's.
        assert seen != {threading.get_ident()}
        assert len(seen) == 1

    asyncio.run(run())


def test_async_conn_kwargs_and_store_functions(tmp_path: Path) -> None:
    async def run() -> None:
        path = tmp_path / "s.db"
        writer = store.connect(path)
        store.start(writer, id="j1", name="n", code="1+1", started_at=1.0)
        db = store.AsyncConn(path)
        try:
            rows = await db.run(store.recent, limit=5)
        finally:
            await db.close()
        assert [r["id"] for r in rows] == ["j1"]

    asyncio.run(run())


def test_async_conn_requires_a_path() -> None:
    # Eager, like store.connect: `serve` must fail at startup, not first request.
    with pytest.raises(ValueError):
        store.AsyncConn(None)  # type: ignore[arg-type]


def test_data_version_moves_only_on_foreign_commit(tmp_path: Path) -> None:
    # The pane bridge's idle gate: reads on the polling connection leave the
    # version unchanged; a commit by ANY other connection (the kernel writing a
    # run) moves it, triggering exactly one re-render.
    path = tmp_path / "v.db"
    poller = store.connect(path)
    before = _data_version(poller)
    store.recent(poller, limit=5)  # own reads never move it
    assert _data_version(poller) == before
    writer = store.connect(path)
    store.start(writer, id="j1", name="", code="1", started_at=1.0)
    assert _data_version(poller) != before
