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


def test_transport_pump_waits_for_initialization_before_draining_mailbox() -> None:
    async def run() -> None:
        box = _box()
        box.add_outbox(content="hello", meta=json.dumps({"resource": "r"}), session="srv")
        set_config(Config(workdir=Path.cwd(), store_path=Path("x"), server_session_id="srv"))

        initialized = anyio.Event()
        send, recv = anyio.create_memory_object_stream[SessionMessage](10)
        async with send, recv, anyio.create_task_group() as tg:
            tg.start_soon(transport.pump_outbox, send, initialized)
            with anyio.move_on_after(0.01) as scope:
                await recv.receive()
            assert scope.cancel_called
            initialized.set()
            msg = await recv.receive()
            tg.cancel_scope.cancel()
        notification = msg.message.root
        assert notification.method == "notifications/claude/channel"
        assert notification.params == {"content": "hello", "meta": {"resource": "r"}}

    anyio.run(run)


def _weave_chat_pump_config(monkeypatch: pytest.MonkeyPatch) -> None:
    set_config(
        Config(
            workdir=Path.cwd(),
            store_path=Path("x"),
            server_session_id="srv",
            channel_delivery="weave-chat",
        )
    )
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:main")
    monkeypatch.setattr(transport, "_OUTBOX_POLL_SECONDS", 0.01)


def test_transport_pump_weave_chat_posts_instead_of_notifying(monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        box = _box()
        box.add_outbox(content="job done", meta=json.dumps({"job_id": "42"}), session="srv")
        _weave_chat_pump_config(monkeypatch)
        posts: list[tuple] = []

        def fake_http(method: str, url: str, *, body: object = None, content: bytes | None = None) -> object:
            posts.append((method, url, body))
            return {"id": body["id"], "seq": 1}

        monkeypatch.setattr(store, "_http_json", fake_http)
        initialized = anyio.Event()
        initialized.set()
        send, recv = anyio.create_memory_object_stream[SessionMessage](10)
        async with send, recv, anyio.create_task_group() as tg:
            tg.start_soon(transport.pump_outbox, send, initialized)
            with anyio.fail_after(2):
                while not posts:
                    await anyio.sleep(0.01)
            # The client is never woken: no channel notification is emitted.
            with anyio.move_on_after(0.05) as scope:
                await recv.receive()
            assert scope.cancel_called
            tg.cancel_scope.cancel()
        method, url, body = posts[0]
        assert (method, url) == ("POST", "http://weave.test/api/chat")
        assert body["to"] == "agent:main"
        assert body["role"] == "user"
        assert body["author"] == "ix-mcp"
        assert body["id"].startswith("msg-ixch-")
        assert body["text"] == '<channel source="ix-mcp" job_id="42">job done</channel>'

    anyio.run(run)


def test_transport_pump_weave_chat_retries_failed_post_with_same_id(monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        box = _box()
        box.add_outbox(content="flaky", meta="{}", session="srv")
        _weave_chat_pump_config(monkeypatch)
        attempts: list[str] = []

        def fake_http(method: str, url: str, *, body: object = None, content: bytes | None = None) -> object:
            attempts.append(body["id"])
            if len(attempts) == 1:
                raise ConnectionError("weave down")
            return {"id": body["id"], "seq": 1}

        monkeypatch.setattr(store, "_http_json", fake_http)
        initialized = anyio.Event()
        initialized.set()
        send, recv = anyio.create_memory_object_stream[SessionMessage](10)
        async with send, recv, anyio.create_task_group() as tg:
            tg.start_soon(transport.pump_outbox, send, initialized)
            with anyio.fail_after(2):
                while len(attempts) < 2:
                    await anyio.sleep(0.01)
            tg.cancel_scope.cancel()
        # The retry reuses the id minted for the row, so an ambiguous failure
        # (response lost after the write landed) cannot double-deliver.
        assert attempts[0] == attempts[1]

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
