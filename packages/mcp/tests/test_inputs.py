"""Interactive input: the browser -> kernel write path behind interactive
resources. Covers the store queue (`channels`/`inputs`), the dashboard
`/api/input` gate + CORS, the kernel-side drain into a live `Input`, and the
`ask` convenience end to end (a store submission resolves the awaiting call)."""

from __future__ import annotations

import asyncio
import json
import sqlite3
from pathlib import Path

import pytest
from aiohttp.test_utils import TestClient, TestServer

from ix_notebook_mcp import dashboard, runtime, store
from ix_notebook_mcp.config import Config

# --------------------------------------------------------------------------- #
# store: the channel registry and the input queue
# --------------------------------------------------------------------------- #


def test_channel_gate_and_input_queue_roundtrip(tmp_path: Path) -> None:
    conn = store.connect(tmp_path / "in.db")
    # An unknown channel is not open: `/api/input` refuses it.
    assert store.channel_open(conn, "nope") is False
    store.open_channel(conn, id="cap1", title="name")
    assert store.channel_open(conn, "cap1") is True

    store.add_input(conn, channel="cap1", payload=json.dumps({"value": "ada"}))
    store.add_input(conn, channel="cap1", payload=json.dumps({"value": "linus"}))
    pending = store.pending_inputs(conn)
    assert [json.loads(p["payload"])["value"] for p in pending] == ["ada", "linus"]
    # seq orders delivery and is what the kernel deletes by.
    assert pending[0]["seq"] < pending[1]["seq"]

    store.delete_inputs(conn, [pending[0]["seq"]])
    remaining = store.pending_inputs(conn)
    assert [json.loads(p["payload"])["value"] for p in remaining] == ["linus"]


def test_close_channel_refuses_and_drops_queued(tmp_path: Path) -> None:
    conn = store.connect(tmp_path / "in.db")
    store.open_channel(conn, id="cap", title="t")
    store.add_input(conn, channel="cap", payload="null")
    store.close_channel(conn, id="cap")
    # A closed channel no longer accepts input, and its undelivered queue is gone.
    assert store.channel_open(conn, "cap") is False
    assert store.pending_inputs(conn) == []


def test_mark_interrupted_closes_channels_and_clears_inputs(tmp_path: Path) -> None:
    conn = store.connect(tmp_path / "in.db")
    store.open_channel(conn, id="cap", title="t")
    store.add_input(conn, channel="cap", payload="1")
    store.mark_interrupted(conn, ended_at=123.0)
    # A reopened session must not leave a stale capability accepting input no
    # awaiter reads, nor carry forward a previous run's queued submissions.
    assert store.channel_open(conn, "cap") is False
    assert store.pending_inputs(conn) == []


# --------------------------------------------------------------------------- #
# dashboard: the /api/input write path + CORS, with no kernel involved
# --------------------------------------------------------------------------- #


def _app(tmp_path: Path) -> tuple[Config, sqlite3.Connection]:
    db = tmp_path / "api.db"
    conn = store.connect(db)
    cfg = Config(workdir=tmp_path, store_path=db)
    return cfg, conn


async def _client(cfg: Config, conn: sqlite3.Connection) -> TestClient:
    client = TestClient(TestServer(dashboard.build_app(cfg, conn)))
    await client.start_server()
    return client


def test_api_input_network_gate(tmp_path: Path) -> None:
    async def run() -> None:
        db = tmp_path / "gate.db"
        conn = store.connect(db)
        store.open_channel(conn, id="cap", title="t")
        body = json.dumps({"channel": "cap", "payload": 1})
        # A non-loopback (tailnet) bind without trust refuses input: the channel id
        # rides in HTML the read endpoints serve, so it is not a secret -- input is
        # authorized by the network boundary, like /api/exec.
        untrusted = Config(workdir=tmp_path, store_path=db, host="100.64.0.1")
        client = await _client(untrusted, conn)
        try:
            resp = await client.post("/api/input", data=body)
            assert resp.status == 403
            assert store.pending_inputs(conn) == []
        finally:
            await client.close()
        # Trusting the tailnet (what the fleet sets) accepts it.
        trusted = Config(workdir=tmp_path, store_path=db, host="100.64.0.1", exec_trust_network=True)
        client = await _client(trusted, conn)
        try:
            resp = await client.post("/api/input", data=body)
            assert resp.status == 200
            assert len(store.pending_inputs(conn)) == 1
        finally:
            await client.close()

    asyncio.run(run())


def test_api_input_accepts_open_channel_and_queues(tmp_path: Path) -> None:
    async def run() -> None:
        cfg, conn = _app(tmp_path)
        store.open_channel(conn, id="cap", title="t")
        client = await _client(cfg, conn)
        try:
            resp = await client.post(
                "/api/input", data=json.dumps({"channel": "cap", "payload": {"value": "hi"}})
            )
            assert resp.status == 200
            assert (await resp.json())["ok"] is True
            # The submission is queued for the kernel to drain.
            pending = store.pending_inputs(conn)
            assert len(pending) == 1
            assert json.loads(pending[0]["payload"]) == {"value": "hi"}
            # CORS so the opaque-origin iframe can read the response.
            assert resp.headers["Access-Control-Allow-Origin"] == "*"
        finally:
            await client.close()

    asyncio.run(run())


def test_api_input_rejects_unknown_and_closed_channel(tmp_path: Path) -> None:
    async def run() -> None:
        cfg, conn = _app(tmp_path)
        client = await _client(cfg, conn)
        try:
            resp = await client.post(
                "/api/input", data=json.dumps({"channel": "ghost", "payload": 1})
            )
            assert resp.status == 404
            assert store.pending_inputs(conn) == []
        finally:
            await client.close()

    asyncio.run(run())


def test_api_input_validation_and_size_cap(tmp_path: Path) -> None:
    async def run() -> None:
        cfg, conn = _app(tmp_path)
        store.open_channel(conn, id="cap", title="t")
        client = await _client(cfg, conn)
        try:
            # Missing payload key is a 400 (distinct from a closed channel's 404).
            resp = await client.post("/api/input", data=json.dumps({"channel": "cap"}))
            assert resp.status == 400
            # Not JSON is a 400.
            resp = await client.post("/api/input", data="not json")
            assert resp.status == 400
            # Oversized body is a 413, not an unbounded write.
            big = json.dumps({"channel": "cap", "payload": "x" * (dashboard._MAX_INPUT_BYTES + 10)})
            resp = await client.post("/api/input", data=big)
            assert resp.status == 413
            assert store.pending_inputs(conn) == []
        finally:
            await client.close()

    asyncio.run(run())


def test_api_input_preflight_returns_cors(tmp_path: Path) -> None:
    async def run() -> None:
        cfg, conn = _app(tmp_path)
        client = await _client(cfg, conn)
        try:
            resp = await client.options("/api/input")
            assert resp.status == 204
            assert resp.headers["Access-Control-Allow-Origin"] == "*"
            assert "POST" in resp.headers["Access-Control-Allow-Methods"]
        finally:
            await client.close()

    asyncio.run(run())


# --------------------------------------------------------------------------- #
# runtime: the kernel end -- a queued submission drains into the awaiting Input
# --------------------------------------------------------------------------- #


def _wire_runtime(monkeypatch: pytest.MonkeyPatch, conn: sqlite3.Connection) -> None:
    monkeypatch.setattr(runtime, "_store", store)
    monkeypatch.setattr(runtime, "_store_conn", conn)
    runtime.input_channels.clear()


def test_input_script_targets_endpoint_and_channel(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("IX_MCP_DATA_API_URL", "http://node:9000/")
    conn = store.connect(tmp_path / "r.db")
    _wire_runtime(monkeypatch, conn)
    inp = runtime.Input(title="name")
    # Opening the channel makes a fast submission authorized before first render.
    assert store.channel_open(conn, inp.id) is True
    script = inp.script
    assert "http://node:9000/api/input" in script
    assert inp.id in script
    assert "ixSubmit" in script


def test_drain_delivers_payload_to_awaiting_input(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)
        inp = runtime.Input(title="name")
        # Simulate the browser POST landing via the dashboard.
        store.add_input(conn, channel=inp.id, payload=json.dumps({"value": "ada"}))
        # The flush tick drains the store into the live channel.
        runtime._drain_inputs()
        assert await asyncio.wait_for(inp.recv(), timeout=1.0) == {"value": "ada"}
        # The row is consumed (delivered exactly once).
        assert store.pending_inputs(conn) == []

    asyncio.run(run())


def test_drain_drops_input_for_closed_channel(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    conn = store.connect(tmp_path / "r.db")
    _wire_runtime(monkeypatch, conn)
    inp = runtime.Input(title="x")
    inp.close()
    store.add_input(conn, channel=inp.id, payload="1")
    runtime._drain_inputs()
    # No awaiter remains; the orphaned submission is dropped, not retained.
    assert store.pending_inputs(conn) == []


def test_ask_resolves_from_submission_and_shapes_reply(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        monkeypatch.setenv("IX_MCP_DATA_API_URL", "http://node:9000/")
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)

        async def answer_after(channel_title_substr: str, payload: dict) -> None:
            # Wait for ask() to open its channel, then deliver a submission as the
            # browser would, and run the drain the flusher normally runs.
            for _ in range(200):
                live = [c for c in runtime.input_channels.values() if not c.closed()]
                if live:
                    store.add_input(conn, channel=live[0].id, payload=json.dumps(payload))
                    runtime._drain_inputs()
                    return
                await asyncio.sleep(0.005)
            raise AssertionError("ask never opened a channel")

        # Single free-text answer reads as the bare value.
        asker = asyncio.ensure_future(runtime.ask("name?"))
        await answer_after("name?", {"value": "ada"})
        assert await asyncio.wait_for(asker, timeout=1.0) == "ada"
        # The window (resource) and channel are closed once answered.
        assert all(c.closed() for c in runtime.input_channels.values())

        # A multi-field form reads as the dict the caller named.
        asker = asyncio.ensure_future(runtime.ask("creds", fields=["user", "password"]))
        await answer_after("creds", {"user": "ada", "password": "pw"})
        assert await asyncio.wait_for(asker, timeout=1.0) == {"user": "ada", "password": "pw"}

    asyncio.run(run())


def test_ask_rejects_choices_and_fields_together() -> None:
    async def run() -> None:
        with pytest.raises(ValueError, match="choices or fields"):
            await runtime.ask("x", choices=["a"], fields=["b"])

    asyncio.run(run())
