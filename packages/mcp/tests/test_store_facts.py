from __future__ import annotations

import os
import time

from ix_notebook_mcp import store


def _capture(monkeypatch):
    calls: list[tuple[str, str, object]] = []

    def fake_json(method: str, url: str, *, body=None, content=None):
        calls.append((method, url, body if body is not None else content))
        if url.endswith("/api/blob"):
            return {"hash": f"h{len(calls):063d}"}
        if url.endswith("/api/query"):
            return {"vars": [], "rows": [], "as_of": 0}
        return [] if isinstance(body, list) else {"seq": 1, "id": "f1"}

    monkeypatch.setattr(store, "_http_json", fake_json)
    monkeypatch.setattr(store, "_http_bytes", lambda method, url, content=None: b"[]")
    return calls


def _drain(conn: store.WeaveStore, timeout: float = 2.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        with conn._cv:
            if not conn._queue:
                return
        time.sleep(0.02)


def test_start_finish_set_session_and_snapshot_emit_fact_shapes(tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:test")
    calls = _capture(monkeypatch)
    conn = store.connect(tmp_path / "session.ixnb")
    store.start(conn, id="abc", name="Example", code="x = 1", started_at=10.0, budget=3.0, topic="t")
    store.finish(conn, id="abc", status="done", ended_at=12.0, output="hello", result="1", error=None, outputs=[{"text": "hello"}], bindings={"x": 1}, namespace=[])
    store.set_session(conn, name="Demo", client="claude")
    store.save_snapshot(conn, created_at=13.0, blob=b"state", names=["x"], skipped=[])
    _drain(conn)
    conn.close()

    fact_batches = [body for _method, url, body in calls if url.endswith("/api/facts")]
    flat = []
    for body in fact_batches:
        flat.extend(body if isinstance(body, list) else [body])
    facts = [item["fact"] for item in flat]

    def fact(entity: str, attr: str, value: object) -> dict:
        # WriteRequest wire shape (crates/protocol/src/api.rs): entity and
        # value are tagged ApiValues; the attr is a plain string. The real
        # server 422s on anything else, so pin the tagged form here.
        t = "bool" if isinstance(value, bool) else "int" if isinstance(value, int) else "float" if isinstance(value, float) else "str"
        return {"entity": {"t": "str", "v": entity}, "attr": attr, "value": {"t": t, "v": value}}

    assert fact("agent:test", "type", "agent") in facts
    assert fact("run:abc", "type", "run") in facts
    assert fact("run:abc", "child_of", "agent:test") in facts
    assert fact("run:abc", "started_ms", 10000) in facts
    assert fact("run:abc", "status", "done") in facts
    assert fact("run:abc", "ended_ms", 12000) in facts
    assert fact("agent:test", "label", "Demo") in facts
    # blob references ride as typed hash values
    assert any(f["entity"]["v"] == "run:abc" and f["attr"] == "code" and f["value"]["t"] == "hash" for f in facts)
    assert any(f["entity"]["v"].startswith("snapshot:") and f["attr"] == "blob" and f["value"]["t"] == "hash" for f in facts)
    assert any(f["entity"]["v"] == "agent:test" and f["attr"] == "snapshot" for f in facts)


def test_weave_url_off_drops_writes_cleanly(tmp_path, monkeypatch, capsys) -> None:
    monkeypatch.setenv("WEAVE_URL", "off")
    monkeypatch.setattr(store, "_WARNED_OFF", False)  # latch is process-global
    conn = store.connect(tmp_path / "off.ixnb")
    store.start(conn, id="abc", name="Example", code="x", started_at=1.0)
    conn.close()
    assert "persistence writes are disabled" in capsys.readouterr().err


def test_read_functions_return_empty_when_disabled(tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "off")
    conn = store.connect(tmp_path / "off.ixnb")
    assert store.recent(conn) == []
    assert store.latest_namespace(conn) == []
    assert store.get(conn, "missing") is None
    assert store.replayable(conn, None) == []
    assert store.live_resources(conn) == []
