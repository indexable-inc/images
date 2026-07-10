"""Session-entity lifecycle facts (weave2 4.6): every MCP connection lands as
its own session entity; disconnect re-asserts status "closed" (latest wins,
never retracted). Hermetic against the weave_stub ABI double."""

from __future__ import annotations

import asyncio
from pathlib import Path

import anyio
import pytest

from ix_notebook_mcp import mailbox, store, transport
from ix_notebook_mcp.config import Config, set_config


def _fact(entity: str, attr: str, value: object) -> dict:
    # WriteRequest wire shape (crates/protocol/src/api.rs): entity and value
    # ride as tagged ApiValues; the attr is a plain string. Same form
    # test_store_facts.py pins -- the real server 422s on anything else.
    t = "bool" if isinstance(value, bool) else "int" if isinstance(value, int) else "float" if isinstance(value, float) else "str"
    return {"entity": {"t": "str", "v": entity}, "attr": attr, "value": {"t": t, "v": value}}


def _wire_facts(fake: object) -> list[dict]:
    return [item["fact"] for item in fake.writes if "fact" in item]


def _statuses(fake: object, sid: str) -> list[object]:
    return [f["value"]["v"] for f in _wire_facts(fake) if f["entity"]["v"] == sid and f["attr"] == "status"]


def _session_entities(fake: object) -> list[str]:
    return sorted({e for (e, a), v in fake.facts.items() if a == "type" and v == "session"})


def test_session_facts_pin_connect_and_disconnect_wire_shapes(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:test")
    conn = store.connect(tmp_path / "session.ixnb")
    store.session_facts(conn, id="abcd1234", status="connected", client="stdio", connected_at=10.0)
    store.session_facts(conn, id="abcd1234", status="closed")
    assert conn.flush(timeout=5.0)
    conn.close()

    sid = "session:abcd1234"
    facts = _wire_facts(fake_weave)
    assert _fact(sid, "type", "session") in facts
    assert _fact(sid, "child_of", "agent:test") in facts
    assert _fact(sid, "on_kernel", conn.kernel) in facts
    # the same kernel entity the store constructor minted, so the dotted
    # kernel -> session edge joins on one node
    assert _fact(conn.kernel, "type", "kernel") in facts
    assert _fact(sid, "client", "stdio") in facts
    assert _fact(sid, "connected_ms", 10000) in facts
    # grey, not gone: close re-asserts status (cardinality one, latest wins),
    # it never retracts
    assert _statuses(fake_weave, sid) == ["connected", "closed"]
    assert all("fact" in item for item in fake_weave.writes)
    assert fake_weave.facts[(sid, "status")] == "closed"


def test_client_upgrade_reasserts_only_the_changed_fact(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    # The stdio transport connects as client="stdio", then renames the client
    # once the initialize handshake declares clientInfo. Same connected_at, so
    # the write-behind dedupe reduces the re-assert to the one changed attr.
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:test")
    conn = store.connect(tmp_path / "session.ixnb")
    store.session_facts(conn, id="abcd1234", status="connected", client="stdio", connected_at=10.0)
    store.session_facts(conn, id="abcd1234", status="connected", client="claude-code", connected_at=10.0)
    assert conn.flush(timeout=5.0)
    conn.close()

    sid = "session:abcd1234"
    per_attr: dict[str, list[object]] = {}
    for f in _wire_facts(fake_weave):
        if f["entity"]["v"] == sid:
            per_attr.setdefault(f["attr"], []).append(f["value"]["v"])
    assert per_attr["client"] == ["stdio", "claude-code"]
    assert per_attr["type"] == ["session"]
    assert per_attr["connected_ms"] == [10000]
    assert per_attr["status"] == ["connected"]


def test_session_facts_are_silent_noop_when_disabled(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "off")
    conn = store.connect(tmp_path / "off.ixnb")
    store.session_facts(conn, id="abcd1234", status="connected", client="http")
    store.session_facts(conn, id="abcd1234", status="closed")
    assert not conn._queue
    conn.close()


def test_stdio_connection_lands_one_session_entity_and_closes_on_eof(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    from mcp.shared.message import SessionMessage

    async def run() -> None:
        mailbox.get_mailbox().reset()
        set_config(Config(workdir=tmp_path, store_path=tmp_path / "s.ixnb"))
        monkeypatch.setattr(transport, "_facts_conn", None)
        server = transport.mcp._mcp_server
        # the held-session mirror is the path under test
        assert transport._can_hold_session(server)
        init_options = server.create_initialization_options(
            experimental_capabilities=transport.CHANNEL_CAPABILITIES,
        )
        client_send, server_recv = anyio.create_memory_object_stream[SessionMessage | Exception](10)
        server_send, client_recv = anyio.create_memory_object_stream[SessionMessage](10)
        async with client_recv, anyio.create_task_group() as tg:
            tg.start_soon(transport._run_with_channel_pump, server, server_recv, server_send, init_options)
            await anyio.sleep(0.05)  # the session is up; its connect facts are queued
            await client_send.aclose()  # client hangs up: the clean-disconnect path

    anyio.run(run)
    conn = transport._facts_conn
    assert conn is not None
    assert conn.flush(timeout=5.0)

    sessions = _session_entities(fake_weave)
    assert len(sessions) == 1  # stdio single-session mode: exactly one entity
    (sid,) = sessions
    assert fake_weave.facts[(sid, "child_of")] == conn.agent
    assert fake_weave.facts[(sid, "on_kernel")] == conn.kernel
    # no initialize handshake happened, so the client stays the transport kind
    assert fake_weave.facts[(sid, "client")] == "stdio"
    assert _statuses(fake_weave, sid) == ["connected", "closed"]
    conn.close()


def test_http_wrapper_lands_one_entity_per_server_run(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    # The SDK's StreamableHTTPSessionManager runs one server.run per MCP
    # session; the wrapper maps each run to its own session entity.
    async def run() -> None:
        set_config(Config(workdir=tmp_path, store_path=tmp_path / "s.ixnb"))
        monkeypatch.setattr(transport, "_facts_conn", None)

        class FakeServer:
            def __init__(self) -> None:
                self.calls: list[tuple[tuple, dict]] = []

            async def run(self, *args: object, **kwargs: object) -> None:
                self.calls.append((args, kwargs))

        server = FakeServer()
        transport._wrap_run_as_session(server)
        # the manager's exact per-session call shape (_handle_stateful_request)
        await server.run("read", "write", "opts", stateless=False)
        await server.run("read", "write", "opts", stateless=False)
        assert server.calls == [(("read", "write", "opts"), {"stateless": False})] * 2

    asyncio.run(run())
    conn = transport._facts_conn
    assert conn is not None
    assert conn.flush(timeout=5.0)

    sessions = _session_entities(fake_weave)
    assert len(sessions) == 2  # multi-client http: one entity per connection
    for sid in sessions:
        assert fake_weave.facts[(sid, "client")] == "http"
        assert fake_weave.facts[(sid, "child_of")] == conn.agent
        assert _statuses(fake_weave, sid) == ["connected", "closed"]
    conn.close()


def test_http_wrapper_closes_the_session_when_run_dies(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    # A run that raises still ends the connection, so the entity flips to
    # closed either way (status is re-asserted, never retracted).
    async def run() -> None:
        set_config(Config(workdir=tmp_path, store_path=tmp_path / "s.ixnb"))
        monkeypatch.setattr(transport, "_facts_conn", None)

        class BoomServer:
            async def run(self, *args: object, **kwargs: object) -> None:
                raise RuntimeError("transport died")

        server = BoomServer()
        transport._wrap_run_as_session(server)
        with pytest.raises(RuntimeError, match="transport died"):
            await server.run("read", "write", "opts", stateless=False)

    asyncio.run(run())
    conn = transport._facts_conn
    assert conn is not None
    assert conn.flush(timeout=5.0)

    sessions = _session_entities(fake_weave)
    assert len(sessions) == 1
    assert _statuses(fake_weave, sessions[0]) == ["connected", "closed"]
    conn.close()
