from __future__ import annotations

import asyncio
import json
from pathlib import Path

import anyio
from aiohttp.test_utils import TestClient, TestServer
from mcp.shared.message import SessionMessage

from ix_notebook_mcp import dashboard, mailbox, runtime, store, tools, transport
from ix_notebook_mcp.config import Config, set_config


def _box() -> mailbox.Mailbox:
    box = mailbox.get_mailbox()
    box.reset()
    return box


def test_outbox_roundtrip_and_session_routing() -> None:
    box = _box()
    box.add_outbox(content="broadcast", meta="{}")
    box.add_outbox(content="mine", meta=json.dumps({"severity": "high"}), session="s1")
    box.add_outbox(content="theirs", meta="{}", session="s2")
    rows = box.take_outbox(session="s1")
    assert [(r["content"], r["session"]) for r in rows] == [("broadcast", ""), ("mine", "s1")]
    assert json.loads(rows[1]["meta"]) == {"severity": "high"}
    assert box.take_outbox(session="s1") == []
    assert [r["content"] for r in box.take_outbox(session="s2")] == ["theirs"]


def test_events_stream_after_seq_and_reset() -> None:
    box = _box()
    assert box.latest_event_seq("res1") == 0
    box.add_event(resource="res1", kind="reply", body=json.dumps({"text": "hi"}))
    start = box.latest_event_seq("res1")
    box.add_event(resource="other", kind="reply", body=json.dumps({"text": "x"}))
    box.add_event(resource="res1", kind="action_result", body=json.dumps({"value": 1}))
    assert [r["kind"] for r in box.events_after("res1", start)] == ["action_result"]
    box.reset()
    assert box.events_after("res1", 0) == []


def _wire_runtime(monkeypatch: pytest.MonkeyPatch, conn: object) -> None:
    monkeypatch.setattr(runtime, "_store", store)
    monkeypatch.setattr(runtime, "_store_conn", conn)
    runtime.input_channels.clear()
    runtime.resources.clear()


def test_notify_queues_event_with_stringified_meta(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        monkeypatch.setenv("WEAVE_URL", "off")
        _box()
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)
        await runtime.notify("build failed", severity="high", run_id=1234)
        rows = store.take_outbox(conn)
        assert len(rows) == 1
        assert rows[0]["content"] == "build failed"
        assert json.loads(rows[0]["meta"]) == {"severity": "high", "run_id": "1234"}
        assert rows[0]["session"] == ""

    asyncio.run(run())


def test_job_finished_event_is_addressed_to_starting_session(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "off")
    _box()
    conn = store.connect(tmp_path / "r.db")
    _wire_runtime(monkeypatch, conn)
    monkeypatch.setenv("IX_MCP_SERVER_SESSION", "srv1")
    job = runtime.Job("1 + 1", name="poll ci", kind="cell", topic="ci", session="abc123")
    job.status = "done"
    job.backgrounded = True
    runtime._notify_job_finished(job)
    rows = store.take_outbox(conn, session="abc123")
    assert [r["session"] for r in rows] == ["abc123"]
    assert rows[0]["content"] == "Background job poll ci finished with status done."


def test_dashboard_sse_streams_mailbox_event(tmp_path: Path) -> None:
    async def run() -> None:
        box = _box()
        cfg = Config(workdir=tmp_path, store_path=tmp_path / "x")
        client = TestClient(TestServer(dashboard.build_app(cfg, mb=box)))
        await client.start_server()
        try:
            resp = await client.get("/api/resources/res/events")
            assert resp.status == 200
            assert await resp.content.readline() == b": connected\n"
            assert await resp.content.readline() == b"\n"
            box.add_event(resource="res", kind="reply", body=json.dumps({"text": "hello"}))
            line = await asyncio.wait_for(resp.content.readline(), timeout=2.0)
            assert b'"kind": "reply"' in line
            assert b'"text": "hello"' in line
        finally:
            await client.close()

    asyncio.run(run())


def test_transport_pump_drains_mailbox() -> None:
    async def run() -> None:
        box = _box()
        box.add_outbox(content="hello", meta=json.dumps({"resource": "r"}), session="srv")
        set_config(Config(workdir=Path.cwd(), store_path=Path("x"), server_session_id="srv"))

        class Session:
            _initialization_state = transport.InitializationState.Initialized

        send, recv = anyio.create_memory_object_stream[SessionMessage](10)
        async with send, recv, anyio.create_task_group() as tg:
            tg.start_soon(transport.pump_outbox, send, Session())
            msg = await recv.receive()
            tg.cancel_scope.cancel()
        notification = msg.message.root
        assert notification.method == "notifications/claude/channel"
        assert notification.params == {"content": "hello", "meta": {"resource": "r"}}

    anyio.run(run)


def test_reply_tool_appends_event(monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        box = _box()
        # Hermetic: no live weave in unit tests; liveness is then unknowable
        # and the reply gate fails open (same-process mailbox resources).
        monkeypatch.setenv("WEAVE_URL", "off")
        monkeypatch.setattr(tools, "_start_dashboard_once", lambda: asyncio.sleep(0))
        monkeypatch.setattr(tools, "_identify_client_once", lambda ctx: asyncio.sleep(0))
        result = await tools.reply("res", "hi", ctx=None)
        assert result[0].text == "sent"
        rows = box.events_after("res", 0)
        assert rows[0]["kind"] == "reply"
        assert json.loads(rows[0]["body"]) == {"text": "hi"}

    asyncio.run(run())
