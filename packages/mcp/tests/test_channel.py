"""The Claude Code channel + interactive resource actions, end to end within one
process: the store outbox/events queues, the kernel's `notify()` and action
dispatch, the transport pump that turns outbox rows into
`notifications/claude/channel` events, the `reply` tool, and the dashboard's SSE
feed a resource's page subscribes to."""

from __future__ import annotations

import asyncio
import json
import sqlite3
import sys
from pathlib import Path

import anyio
import pytest
from aiohttp.test_utils import TestClient, TestServer

from ix_notebook_mcp import dashboard, runtime, store, transport
from ix_notebook_mcp.config import Config

# --------------------------------------------------------------------------- #
# store: the outbox and event queues
# --------------------------------------------------------------------------- #


def test_outbox_roundtrip_consumes_in_order(tmp_path: Path) -> None:
    conn = store.connect(tmp_path / "c.db")
    store.add_outbox(conn, content="first", meta="{}")
    store.add_outbox(conn, content="second", meta=json.dumps({"severity": "high"}))
    rows = store.take_outbox(conn)
    assert [r["content"] for r in rows] == ["first", "second"]
    assert json.loads(rows[1]["meta"]) == {"severity": "high"}
    # take consumes: a second drain sees nothing (an event is emitted once).
    assert store.take_outbox(conn) == []


def test_events_stream_after_seq_and_live_gate(tmp_path: Path) -> None:
    conn = store.connect(tmp_path / "c.db")
    assert store.latest_event_seq(conn, "res1") == 0
    store.add_event(conn, resource="res1", kind="reply", body=json.dumps({"text": "hi"}))
    store.add_event(conn, resource="other", kind="reply", body=json.dumps({"text": "x"}))
    start = store.latest_event_seq(conn, "res1")
    store.add_event(conn, resource="res1", kind="action_result", body=json.dumps({"value": 1}))
    rows = store.events_after(conn, "res1", start)
    # Only res1's rows past the subscription point, never another resource's.
    assert [r["kind"] for r in rows] == ["action_result"]

    # The reply tool's gate: only a not-closed resource is live.
    assert store.resource_live(conn, "res1") is False
    store.upsert_resource(
        conn, id="res1", title="t", kind="html", html="", status="live", created_at=1.0, updated_at=1.0
    )
    assert store.resource_live(conn, "res1") is True
    store.close_resource(conn, id="res1", updated_at=2.0)
    assert store.resource_live(conn, "res1") is False


def test_mark_interrupted_clears_outbox_and_events(tmp_path: Path) -> None:
    conn = store.connect(tmp_path / "c.db")
    store.add_outbox(conn, content="stale", meta="{}")
    store.add_event(conn, resource="r", kind="reply", body="{}")
    store.mark_interrupted(conn, ended_at=123.0)
    # A reopened session must not fire a dead kernel's pushes or replay its feed.
    assert store.take_outbox(conn) == []
    assert store.events_after(conn, "r", 0) == []


# --------------------------------------------------------------------------- #
# runtime: notify() and interactive resource actions (the kernel end)
# --------------------------------------------------------------------------- #


def _wire_runtime(monkeypatch: pytest.MonkeyPatch, conn: sqlite3.Connection) -> None:
    monkeypatch.setattr(runtime, "_store", store)
    monkeypatch.setattr(runtime, "_store_conn", conn)
    runtime.input_channels.clear()
    runtime.resources.clear()


def test_notify_queues_event_with_stringified_meta(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)
        await runtime.notify("build failed", severity="high", run_id=1234)
        rows = store.take_outbox(conn)
        assert len(rows) == 1
        assert rows[0]["content"] == "build failed"
        # Values are stringified: they become <channel> tag attributes.
        assert json.loads(rows[0]["meta"]) == {"severity": "high", "run_id": "1234"}

    asyncio.run(run())


def test_notify_rejects_non_identifier_meta_keys(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)
        # Claude Code silently drops hyphenated keys; we fail loudly at source.
        with pytest.raises(ValueError, match="meta keys"):
            await runtime.notify("x", **{"run-id": "1"})
        assert store.take_outbox(conn) == []

    asyncio.run(run())


def test_notify_without_store_raises(monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        monkeypatch.setattr(runtime, "_store", None)
        monkeypatch.setattr(runtime, "_store_conn", None)
        with pytest.raises(RuntimeError, match="no store"):
            await runtime.notify("x")

    asyncio.run(run())


def test_interactive_resource_injects_wiring_script(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        monkeypatch.setenv("IX_MCP_DATA_API_URL", "http://node:9000/")
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)
        res = runtime.register_resource(
            render=lambda: "<button>go</button>", id="panel", actions={"go": lambda p: p}
        )
        html = await res.render_html()
        # The page gets ix.act/ix.events without including anything itself.
        assert "x.act=function" in html
        assert "x.events=function" in html
        assert "http://node:9000/api/input" in html
        assert "http://node:9000/api/resources/panel/events" in html
        # Pin the ix.act POST body shape: the kernel dispatcher reads exactly these
        # keys (action/call/payload), so a rename in the injected script that this
        # substring misses would break the real browser path while the dispatch
        # test (which hand-builds the same dict) still passed.
        assert "body:JSON.stringify({channel:C,payload:{action:a,call:id," in html
        assert html.endswith("<button>go</button>")
        # A plain resource stays untouched.
        plain = runtime.register_resource(render=lambda: "<p>hi</p>", id="plain")
        assert await plain.render_html() == "<p>hi</p>"
        res.close()

    asyncio.run(run())


def test_interactive_resource_id_must_be_script_safe(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)
        # An id carrying </script> would break out of the injected <script> (XSS in
        # the pane) and a slash would miss the SSE route, so an interactive id is
        # restricted to [A-Za-z0-9._-].
        with pytest.raises(ValueError, match="interactive resource id"):
            runtime.register_resource(
                render=lambda: "x", id="a</script><img src=x>", actions={"go": lambda p: p}
            )
        with pytest.raises(ValueError, match="interactive resource id"):
            runtime.register_resource(render=lambda: "x", id="a/b", actions={"go": lambda p: p})
        # A plain (non-interactive) resource never reaches the script/route, so it
        # still accepts any id.
        plain = runtime.register_resource(render=lambda: "x", id="a/b weird")
        assert plain.id == "a/b weird"
        plain.close()

    asyncio.run(run())


def test_closed_before_first_sweep_keeps_final_resource(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)
        res = runtime.register_resource(render=lambda: "<p>terminal</p>", id="blink", title="blink")
        res.close()

        await runtime._sweep_resources()

        row = conn.execute(
            "SELECT title, html, status FROM resources WHERE id = ?",
            ("blink",),
        ).fetchone()
        assert row is not None
        assert row[0] == "blink"
        assert row[1] == "<p>terminal</p>"
        assert row[2] == "closed"
        assert "blink" not in runtime.resources

    asyncio.run(run())


def test_action_dispatch_runs_handler_and_streams_result(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)

        async def double(payload: dict) -> dict:
            return {"doubled": payload["n"] * 2}

        def boom(_payload: object) -> None:
            raise RuntimeError("kaput")

        res = runtime.register_resource(
            render=lambda: "x", id="panel", actions={"double": double, "boom": boom}
        )
        channel = res._action_channel
        assert channel is not None

        async def act(name: str, payload: object) -> None:
            # Simulate the page's ix.act landing via /api/input, then the drain.
            store.add_input(
                conn,
                channel=channel.id,
                payload=json.dumps({"action": name, "call": "c1", "payload": payload}),
            )
            runtime._drain_inputs()

        async def feed_after(start: int) -> list[dict]:
            for _ in range(200):
                rows = store.events_after(conn, "panel", start)
                if rows:
                    return rows
                await asyncio.sleep(0.005)
            raise AssertionError("no event arrived")

        seq = store.latest_event_seq(conn, "panel")
        await act("double", {"n": 21})
        rows = await feed_after(seq)
        body = json.loads(rows[-1]["body"])
        assert rows[-1]["kind"] == "action_result"
        assert body == {"action": "double", "call": "c1", "value": {"doubled": 42}}

        # A raising handler streams an error event, and the dispatcher survives.
        seq = store.latest_event_seq(conn, "panel")
        await act("boom", None)
        rows = await feed_after(seq)
        assert rows[-1]["kind"] == "error"
        assert "kaput" in json.loads(rows[-1]["body"])["error"]

        # An unknown action is an error event too, never a silent drop.
        seq = store.latest_event_seq(conn, "panel")
        await act("ghost", None)
        rows = await feed_after(seq)
        assert "no such action" in json.loads(rows[-1]["body"])["error"]

        # close() tears down the channel so a stale page cannot queue more work.
        res.close()
        assert channel.closed()

    asyncio.run(run())


def test_reregistering_id_closes_previous_action_channel(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        conn = store.connect(tmp_path / "r.db")
        _wire_runtime(monkeypatch, conn)
        first = runtime.register_resource(render=lambda: "a", id="panel", actions={"a": lambda p: p})
        old_channel = first._action_channel
        assert old_channel is not None
        second = runtime.register_resource(render=lambda: "b", id="panel", actions={"b": lambda p: p})
        # The replaced resource's channel is closed; the new one is live.
        assert old_channel.closed()
        assert first.closed()
        assert second._action_channel is not None
        assert not second._action_channel.closed()
        second.close()

    asyncio.run(run())


# --------------------------------------------------------------------------- #
# transport: the outbox pump emits notifications/claude/channel
# --------------------------------------------------------------------------- #


def test_channel_capability_is_advertised() -> None:
    from ix_notebook_mcp.tools import mcp

    opts = mcp._mcp_server.create_initialization_options(
        experimental_capabilities=transport.CHANNEL_CAPABILITIES
    )
    assert opts.capabilities.experimental is not None
    assert "claude/channel" in opts.capabilities.experimental


class _FakeSession:
    """Just enough of ServerSession for the pump's initialized gate."""

    def __init__(self, *, initialized: bool) -> None:
        from mcp.server.session import InitializationState

        self._initialization_state = (
            InitializationState.Initialized if initialized else InitializationState.NotInitialized
        )


def test_pump_outbox_emits_channel_notifications(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        db = tmp_path / "p.db"
        conn = store.connect(db)
        cfg = Config(workdir=tmp_path, store_path=db)
        monkeypatch.setattr("ix_notebook_mcp.transport.config", lambda: cfg)
        store.add_outbox(conn, content="hello agent", meta=json.dumps({"severity": "high"}))
        send, receive = anyio.create_memory_object_stream(8)
        pump = asyncio.ensure_future(transport.pump_outbox(send, _FakeSession(initialized=True)))
        try:
            message = await asyncio.wait_for(receive.receive(), timeout=5.0)
        finally:
            pump.cancel()
        wire = message.message.root
        assert wire.method == "notifications/claude/channel"
        assert wire.params == {"content": "hello agent", "meta": {"severity": "high"}}
        # The row was consumed: a redelivery cannot happen.
        assert store.take_outbox(conn) == []

    asyncio.run(run())


def test_pump_outbox_holds_events_until_initialized(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        db = tmp_path / "p.db"
        conn = store.connect(db)
        cfg = Config(workdir=tmp_path, store_path=db)
        monkeypatch.setattr("ix_notebook_mcp.transport.config", lambda: cfg)
        store.add_outbox(conn, content="early", meta="{}")
        send, receive = anyio.create_memory_object_stream(8)
        # An un-initialized session must not have the row emitted: the MCP lifecycle
        # forbids server notifications before `initialized`, and the row must be held
        # (not dropped) for later delivery.
        pump = asyncio.ensure_future(transport.pump_outbox(send, _FakeSession(initialized=False)))
        try:
            with pytest.raises(asyncio.TimeoutError):
                await asyncio.wait_for(receive.receive(), timeout=1.0)
            # The event is still queued, waiting for the handshake.
            assert len(store.take_outbox(conn)) == 1
        finally:
            pump.cancel()

    asyncio.run(run())


# --------------------------------------------------------------------------- #
# dashboard: the SSE feed a resource's page subscribes to
# --------------------------------------------------------------------------- #


def test_sse_streams_new_events_only(tmp_path: Path) -> None:
    async def run() -> None:
        db = tmp_path / "sse.db"
        conn = store.connect(db)
        cfg = Config(workdir=tmp_path, store_path=db)
        # History from before the subscription must not replay.
        store.add_event(conn, resource="panel", kind="reply", body=json.dumps({"text": "old"}))
        client = TestClient(TestServer(dashboard.build_app(cfg, conn)))
        await client.start_server()
        try:
            async with client.get("/api/resources/panel/events") as resp:
                assert resp.status == 200
                assert resp.headers["Content-Type"].startswith("text/event-stream")
                assert resp.headers["Access-Control-Allow-Origin"] == "*"
                # The comment frame arrives first (EventSource open).
                line = await asyncio.wait_for(resp.content.readline(), timeout=5.0)
                assert line.startswith(b":")
                store.add_event(conn, resource="panel", kind="reply", body=json.dumps({"text": "new"}))
                while True:
                    line = await asyncio.wait_for(resp.content.readline(), timeout=5.0)
                    if line.startswith(b"data: "):
                        break
                event = json.loads(line[len(b"data: "):])
                assert event["kind"] == "reply"
                assert event["text"] == "new"
        finally:
            await client.close()

    asyncio.run(run())


# --------------------------------------------------------------------------- #
# tools: the reply tool writes to the feed, gated on a live resource
# --------------------------------------------------------------------------- #


def test_reply_tool_appends_event_for_live_resource(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    async def run() -> None:
        from mcp.shared.exceptions import McpError

        from ix_notebook_mcp import tools

        db = tmp_path / "reply.db"
        conn = store.connect(db)
        cfg = Config(workdir=tmp_path, store_path=db)
        monkeypatch.setattr("ix_notebook_mcp.tools.config", lambda: cfg)
        monkeypatch.setattr(tools, "_reply_conn", None)
        monkeypatch.setattr(tools, "_dashboard_started", True)

        # An unknown/closed resource is refused loudly, and nothing is written.
        with pytest.raises(McpError, match="no live resource"):
            await tools.reply(resource="ghost", text="hi")
        assert store.events_after(conn, "ghost", 0) == []

        store.upsert_resource(
            conn, id="panel", title="t", kind="html", html="", status="live", created_at=1.0, updated_at=1.0
        )
        out = await tools.reply(resource="panel", text="deployed ✓")
        assert out[0].text == "sent"
        rows = store.events_after(conn, "panel", 0)
        assert [(r["kind"], json.loads(r["body"])["text"]) for r in rows] == [("reply", "deployed ✓")]

    asyncio.run(run())


def test_pr_watch_tool_returns_header_with_slugged_resource(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """pr_watch's happy path up to the header (regression: NameError on `re`, #1900)."""

    async def run() -> None:
        from ix_notebook_mcp import tools

        cfg = Config(workdir=tmp_path, store_path=tmp_path / "w.db")
        monkeypatch.setattr("ix_notebook_mcp.tools.config", lambda: cfg)

        async def gate(*_args: object, **_kwargs: object) -> None:
            return None

        monkeypatch.setattr(tools, "_start_dashboard_once", gate)
        monkeypatch.setattr(tools, "_identify_client_once", gate)
        monkeypatch.setattr(tools, "_require_acting_gates", gate)

        class FakeKernel:
            async def python_exec(
                self,
                code: str,
                budget: float,
                intent: str,
                *,
                session: str | None = None,
                topic: str | None = None,
            ) -> tuple[list, dict]:
                assert "watch_pr" in code
                return [], {"id": "ab12", "status": "running", "running": True, "elapsed_s": 0.1}

        monkeypatch.setattr(tools, "current_kernel", FakeKernel)

        out = await tools.pr_watch("https://github.com/o/r/pull/1856", cwd=str(tmp_path))
        # pr_watch now returns a CallToolResult (MCP Apps: the human view rides
        # its _meta); the model-facing blocks live on .content, same as before.
        header = json.loads(out.content[0].text)
        # The URL is slugged into a resource id safe for the dashboard route.
        assert header["resource"] == "pr-https-github.com-o-r-pull-1856"
        assert header["job"] == "ab12"

    asyncio.run(run())


def test_acting_gates_report_both_unmet_gates_in_one_error(monkeypatch: pytest.MonkeyPatch) -> None:
    """A fresh session's first acting call names BOTH unmet gates in its one
    rejection, instead of tripping session-name and then topic serially at a
    full tool round trip each (#1983)."""
    from mcp.shared.exceptions import McpError

    from ix_notebook_mcp import tools

    monkeypatch.delenv("IX_MCP_REQUIRE_SESSION_NAME", raising=False)
    monkeypatch.delenv("IX_MCP_REQUIRE_TOPIC", raising=False)
    monkeypatch.setattr(tools, "_solo_session_name", None)
    monkeypatch.setattr(tools, "_solo_topic", None)

    with pytest.raises(McpError) as excinfo:
        asyncio.run(tools._require_acting_gates(None, intent="count rows per host"))
    message = str(excinfo.value)
    assert "session_set_name" in message
    assert "topic_set" in message
    assert "'count rows per host'" in message

    # Once named, only the topic gate remains -- and it alone is reported.
    monkeypatch.setattr(tools, "_solo_session_name", "count rows per host")
    with pytest.raises(McpError, match="topic_set") as excinfo:
        asyncio.run(tools._require_acting_gates(None, intent="count rows per host"))
    assert "session_set_name" not in str(excinfo.value)

    # Both gates satisfied: the check passes silently.
    monkeypatch.setattr(tools, "_solo_topic", "row counts")
    asyncio.run(tools._require_acting_gates(None, intent="count rows per host"))


def test_pr_resource_html_renders_every_check_state(monkeypatch: pytest.MonkeyPatch) -> None:
    """`_pr_resource_html` slugs each check's raw state into a CSS class with
    `re.sub` (regression: NameError on `re`, #1900/#1933 -- this path is NOT
    covered by test_pr_watch_tool_returns_header_with_slugged_resource, which
    mocks python_exec and never runs runtime.watch_pr's own body)."""
    state = {
        "pr": "1856",
        "title": "fix: something",
        "url": "https://github.com/o/r/pull/1856",
        "status": "open",
        "merge_state": "clean",
        "elapsed": "1m 2s",
        "auto_merge": "auto merge on",
        "error": "",
        "checks": [
            {"name": "build", "conclusion": "SUCCESS", "startedAt": "", "completedAt": ""},
            # A conclusion with characters outside [a-z0-9_-] (a space) exercises
            # the re.sub slugging rather than a value that already happens to be safe.
            {"name": "action required check", "conclusion": "ACTION_REQUIRED", "startedAt": "", "completedAt": ""},
        ],
    }
    html = runtime._pr_resource_html(state)
    assert '<span class="state success">success</span>' in html
    assert '<span class="state action_required">action_required</span>' in html


class _FakeFrame:
    """Stands in for the `pl.DataFrame` a real `nu()` call returns; watch_pr only
    calls `.to_dicts()` on the refresh-loop result."""

    def __init__(self, rows: list[dict[str, object]]) -> None:
        self._rows = rows

    def to_dicts(self) -> list[dict[str, object]]:
        return self._rows


class _FakeNu:
    """A stand-in for the bundled `nu` module: `watch_pr` does `import nu as
    nu_call` then calls it directly (`nu_call(code, ...)`), so this needs to be
    an instance whose TYPE defines `__call__` -- an attribute set on a plain
    instance would not make `instance(...)` callable."""

    async def __call__(
        self, code: str, *, cwd: str | None = None, env: dict[str, str] | None = None, timeout: float = 60
    ) -> _FakeFrame:
        assert "gh pr view" in code
        return _FakeFrame(
            [
                {
                    "number": 1856,
                    "title": "fix: something",
                    "state": "MERGED",
                    "mergeStateStatus": "CLEAN",
                    "statusCheckRollup": [{"name": "build", "conclusion": "SUCCESS"}],
                    "url": "https://github.com/o/r/pull/1856",
                    "autoMergeRequest": None,
                    "isDraft": False,
                    "reviewDecision": "APPROVED",
                }
            ]
        )


def test_watch_pr_slugs_resource_id_and_renders_without_nameerror(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Drives `runtime.watch_pr` itself (not the mocked-kernel `tools.pr_watch`
    path), with `nu`/`notify` stubbed, so the `re.sub` calls at both the
    resource-id slug (runtime.py) and inside `_pr_resource_html` actually run.
    Regression: NameError on `re` (#1900, reopened as #1933)."""

    async def run() -> None:
        monkeypatch.setitem(sys.modules, "nu", _FakeNu())

        notified: list[tuple[str, dict[str, object]]] = []

        async def fake_notify(content: str, **meta: object) -> None:
            notified.append((content, meta))

        monkeypatch.setattr(runtime, "notify", fake_notify)

        result = await runtime.watch_pr(
            "https://github.com/o/r/pull/1856",
            cwd=str(tmp_path),
            auto_merge=False,
        )

        assert result == {"state": "MERGED", "url": "https://github.com/o/r/pull/1856", "checks": 1}
        # The resource id is the same slugged form pr_watch's header reports.
        resource = runtime.resources["pr-https-github.com-o-r-pull-1856"]
        assert resource.closed()
        assert notified
        assert notified[0][1]["resource"] == "pr-https-github.com-o-r-pull-1856"

    asyncio.run(run())
