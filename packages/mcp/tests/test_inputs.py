from __future__ import annotations

import asyncio
import json
from pathlib import Path

from aiohttp.test_utils import TestClient, TestServer

from ix_notebook_mcp import dashboard, mailbox, runtime, store
from ix_notebook_mcp.config import Config


def _box() -> mailbox.Mailbox:
    box = mailbox.get_mailbox()
    box.reset()
    return box


async def _client(cfg: Config) -> TestClient:
    client = TestClient(TestServer(dashboard.build_app(cfg, mb=mailbox.get_mailbox())))
    await client.start_server()
    return client


def test_api_input_network_gate(tmp_path: Path) -> None:
    async def run() -> None:
        box = _box()
        box.open_channel(id="cap", title="t")
        body = json.dumps({"channel": "cap", "payload": 1})
        client = await _client(Config(workdir=tmp_path, store_path=tmp_path / "x", host="100.64.0.1"))
        try:
            resp = await client.post("/api/input", data=body)
            assert resp.status == 403
            assert box.pending_inputs() == []
        finally:
            await client.close()
        client = await _client(Config(workdir=tmp_path, store_path=tmp_path / "x", host="100.64.0.1", exec_trust_network=True))
        try:
            resp = await client.post("/api/input", data=body)
            assert resp.status == 200
            assert len(box.pending_inputs()) == 1
        finally:
            await client.close()

    asyncio.run(run())


def test_api_input_accepts_open_channel_and_queues(tmp_path: Path) -> None:
    async def run() -> None:
        box = _box()
        box.open_channel(id="cap", title="t")
        client = await _client(Config(workdir=tmp_path, store_path=tmp_path / "x"))
        try:
            resp = await client.post("/api/input", data=json.dumps({"channel": "cap", "payload": {"value": "hi"}}))
            assert resp.status == 200
            assert (await resp.json())["ok"] is True
            pending = box.pending_inputs()
            assert json.loads(pending[0]["payload"]) == {"value": "hi"}
            assert resp.headers["Access-Control-Allow-Origin"] == "*"
        finally:
            await client.close()

    asyncio.run(run())


def test_api_input_rejects_unknown_and_closed_channel(tmp_path: Path) -> None:
    async def run() -> None:
        _box()
        client = await _client(Config(workdir=tmp_path, store_path=tmp_path / "x"))
        try:
            resp = await client.post("/api/input", data=json.dumps({"channel": "ghost", "payload": 1}))
            assert resp.status == 404
        finally:
            await client.close()

    asyncio.run(run())


def test_api_input_validation_and_size_cap(tmp_path: Path) -> None:
    async def run() -> None:
        box = _box()
        box.open_channel(id="cap", title="t")
        client = await _client(Config(workdir=tmp_path, store_path=tmp_path / "x"))
        try:
            assert (await client.post("/api/input", data=json.dumps({"channel": "cap"}))).status == 400
            assert (await client.post("/api/input", data="not json")).status == 400
            big = json.dumps({"channel": "cap", "payload": "x" * (dashboard._MAX_INPUT_BYTES + 10)})
            assert (await client.post("/api/input", data=big)).status == 413
            assert box.pending_inputs() == []
        finally:
            await client.close()

    asyncio.run(run())


def test_api_input_preflight_returns_cors(tmp_path: Path) -> None:
    async def run() -> None:
        _box()
        client = await _client(Config(workdir=tmp_path, store_path=tmp_path / "x"))
        try:
            resp = await client.options("/api/input")
            assert resp.status == 204
            assert resp.headers["Access-Control-Allow-Origin"] == "*"
            assert "POST" in resp.headers["Access-Control-Allow-Methods"]
        finally:
            await client.close()

    asyncio.run(run())


def _wire_runtime(monkeypatch: pytest.MonkeyPatch, conn: object) -> None:
    monkeypatch.setattr(runtime, "_store", store)
    monkeypatch.setattr(runtime, "_store_conn", conn)
    runtime.input_channels.clear()


def test_input_script_targets_endpoint_and_channel(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("IX_MCP_DATA_API_URL", "http://node:9000/")
    monkeypatch.setenv("WEAVE_URL", "off")
    mailbox.get_mailbox().reset()
    conn = store.connect(tmp_path / "r.db")
    _wire_runtime(monkeypatch, conn)
    inp = runtime.Input(title="name")
    assert store.channel_open(conn, inp.id) is True
    assert "http://node:9000/api/input" in inp.script
    assert inp.id in inp.script


def test_drain_delivers_payload_to_awaiting_input(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        monkeypatch.setenv("WEAVE_URL", "off")
        mailbox.get_mailbox().reset()
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)
        inp = runtime.Input(title="name")
        store.add_input(conn, channel=inp.id, payload=json.dumps({"value": "ada"}))
        runtime._drain_inputs()
        assert await asyncio.wait_for(inp.recv(), timeout=1.0) == {"value": "ada"}
        assert store.pending_inputs(conn) == []

    asyncio.run(run())
