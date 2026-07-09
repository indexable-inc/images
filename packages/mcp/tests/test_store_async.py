"""The store's async facade: every call runs on one private worker thread, off
the caller's event loop."""

from __future__ import annotations

import asyncio
import threading
from pathlib import Path

import pytest

from ix_notebook_mcp import store


def test_async_conn_runs_off_loop_on_one_thread(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "off")

    async def run() -> None:
        db = store.AsyncConn(tmp_path / "a.ixnb")
        seen: set[int] = set()

        def probe(conn: store.WeaveStore, value: int) -> int:
            seen.add(threading.get_ident())
            # The handle is live and usable where the call runs.
            assert isinstance(conn, store.WeaveStore)
            return value

        try:
            assert [await db.run(probe, n) for n in range(3)] == [0, 1, 2]
        finally:
            await db.close()
        # Confined: every call landed on the same worker thread, never the loop's.
        assert seen != {threading.get_ident()}
        assert len(seen) == 1

    asyncio.run(run())


def test_async_conn_kwargs_and_store_functions(tmp_path: Path, monkeypatch) -> None:
    """store.recent through the facade translates weave query rows into the
    execution dict shape callers have always consumed."""
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:test")

    def fake_json(method: str, url: str, *, body=None, content=None):
        if url.endswith("/api/query"):
            # pivot rows for one finished run child of agent:test
            rows = [
                [{"t": "str", "v": "run:j1"}, {"t": "str", "v": "type"}, {"t": "str", "v": "run"}],
                [{"t": "str", "v": "run:j1"}, {"t": "str", "v": "desc"}, {"t": "str", "v": "n"}],
                [{"t": "str", "v": "run:j1"}, {"t": "str", "v": "status"}, {"t": "str", "v": "done"}],
                [{"t": "str", "v": "run:j1"}, {"t": "str", "v": "started_ms"}, {"t": "int", "v": 1000}],
            ]
            return {"vars": ["E", "A", "V"], "rows": rows, "as_of": 9}
        return {"seq": 1, "id": "f1"}

    monkeypatch.setattr(store, "_http_json", fake_json)

    async def run() -> None:
        db = store.AsyncConn(tmp_path / "s.ixnb")
        try:
            rows = await db.run(store.recent, limit=5)
        finally:
            await db.close()
        assert [r["id"] for r in rows] == ["j1"]
        assert rows[0]["status"] == "done"
        assert rows[0]["name"] == "n"
        assert rows[0]["started_at"] == 1.0

    asyncio.run(run())


def test_async_conn_requires_a_path() -> None:
    # Eager, like store.connect: `serve` must fail at startup, not first request.
    with pytest.raises(ValueError, match="store path is required"):
        store.AsyncConn(None)  # type: ignore[arg-type]
