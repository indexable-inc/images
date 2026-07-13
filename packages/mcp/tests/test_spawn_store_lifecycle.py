"""A spawned job starts and finishes one process entity, never a phantom run."""

from __future__ import annotations

from pathlib import Path

import pytest

from ix_notebook_mcp import store


def test_spawn_finish_updates_its_process_without_creating_a_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls: list[tuple[str, str, object]] = []

    def fake_json(
        method: str, url: str, *, body: object = None, content: bytes | None = None
    ) -> object:
        calls.append((method, url, body if body is not None else content))
        if url.endswith("/api/blob"):
            return {"hash": f"h{len(calls):063d}"}
        if url.endswith("/api/query"):
            return {"vars": [], "rows": [], "as_of": 0}
        return [] if isinstance(body, list) else {"seq": 1, "id": "f1"}

    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:test")
    monkeypatch.setattr(store, "_http_json", fake_json)
    monkeypatch.setattr(store, "_http_bytes", lambda method, url, content=None: b"[]")

    conn = store.connect(tmp_path / "spawn.ixnb")
    store.start(
        conn,
        id="job-1",
        name="delegation",
        code="await work()",
        started_at=10.0,
        kind="spawn",
    )
    store.finish(
        conn,
        id="job-1",
        kind="spawn",
        status="done",
        ended_at=12.0,
        output="",
        result="finished",
        error=None,
    )
    assert conn.flush(timeout=2.0)
    conn.close()

    fact_batches = [body for _method, url, body in calls if url.endswith("/api/facts")]
    facts = [
        item["fact"]
        for body in fact_batches
        for item in (body if isinstance(body, list) else [body])
    ]
    entities = [fact["entity"]["v"] for fact in facts]
    assert "proc:job-1" in entities
    assert "run:job-1" not in entities
    assert any(
        fact["entity"]["v"] == "proc:job-1"
        and fact["attr"] == "status"
        and fact["value"] == {"t": "str", "v": "done"}
        for fact in facts
    )
    assert any(
        fact["entity"]["v"] == "proc:job-1" and fact["attr"] == "ended_ms"
        for fact in facts
    )
