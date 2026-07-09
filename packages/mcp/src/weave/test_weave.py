from __future__ import annotations

import asyncio
import os
import sys
from pathlib import Path
from typing import Any

import httpx
import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "weave"))
sys.path.insert(0, str(ROOT.parent))

import weave
from weave import supervisor


def run(coro: Any) -> Any:
    return asyncio.run(coro)


def install_transport(monkeypatch: pytest.MonkeyPatch, handler: Any) -> list[Any]:
    seen: list[Any] = []

    def wrapped(req: httpx.Request) -> httpx.Response:
        seen.append(req)
        return handler(req)

    transport = httpx.MockTransport(wrapped)
    monkeypatch.setattr(
        weave,
        "_client",
        lambda **kw: httpx.AsyncClient(transport=transport, base_url="http://weave.test", **kw),
    )
    return seen


def test_value_mapping_bool_before_int_and_hashref(monkeypatch: pytest.MonkeyPatch) -> None:
    bodies: list[Any] = []

    def handler(req: httpx.Request) -> httpx.Response:
        bodies.append(req.read())
        return httpx.Response(200, json={"seq": 1, "id": "f1"})

    install_transport(monkeypatch, handler)
    h = "a" * 64
    run(weave.assert_fact("e", "s", "x"))
    run(weave.assert_fact("e", "b", True))
    run(weave.assert_fact("e", "i", 3))
    run(weave.assert_fact("e", "f", 1.5))
    run(weave.assert_fact("e", "h", weave.hashref(f"blake3:{h}")))
    payloads = [weave.json.loads(b) for b in bodies]
    assert [p["fact"]["value"]["t"] for p in payloads] == ["str", "bool", "int", "float", "hash"]
    assert payloads[-1]["fact"]["value"]["v"] == h
    # the entity rides as a tagged ApiValue too (the real server 422s on bare strings)
    assert payloads[0]["fact"]["entity"] == {"t": "str", "v": "e"}
    with pytest.raises(TypeError):
        run(weave.assert_fact("e", "bytes", b"nope"))


def test_assert_facts_batches_500(monkeypatch: pytest.MonkeyPatch) -> None:
    sizes: list[int] = []

    def handler(req: httpx.Request) -> httpx.Response:
        batch = weave.json.loads(req.read())
        sizes.append(len(batch))
        return httpx.Response(200, json=[{"seq": i, "id": f"f{i}"} for i in range(len(batch))])

    install_transport(monkeypatch, handler)
    facts = [(f"e{i}", "a", i) for i in range(1200)]
    out = run(weave.assert_facts(facts))
    assert sizes == [500, 500, 200]
    assert len(out) == 1200


def test_query_unwrap_and_frame(monkeypatch: pytest.MonkeyPatch) -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        return httpx.Response(
            200,
            json={
                "vars": ["E", "N", "H"],
                "rows": [[{"t": "str", "v": "e"}, {"t": "int", "v": 7}, {"t": "hash", "v": "b" * 64}]],
                "as_of": 9,
            },
        )

    install_transport(monkeypatch, handler)
    res = run(weave.query("?- x(E).", as_of=9))
    assert res == {"vars": ["E", "N", "H"], "rows": [["e", 7, "blake3:" + "b" * 64]], "as_of": 9}
    assert res.frame().columns == ["E", "N", "H"]


def test_watch_diffs_rows_keyed_by_first_column(monkeypatch: pytest.MonkeyPatch) -> None:
    query_responses = [
        {"vars": ["E", "V"], "rows": [[{"t": "str", "v": "a"}, {"t": "int", "v": 1}]], "as_of": 1},
        {"vars": ["E", "V"], "rows": [[{"t": "str", "v": "a"}, {"t": "int", "v": 2}], [{"t": "str", "v": "b"}, {"t": "int", "v": 1}]], "as_of": 2},
    ]

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/api/events":
            return httpx.Response(200, stream=httpx.ByteStream(b"data: {\"head\":1}\n\ndata: {\"head\":2}\n\n"))
        return httpx.Response(200, json=query_responses.pop(0))

    install_transport(monkeypatch, handler)

    async def collect() -> list[Any]:
        out = []
        async for batch in weave.watch("?- x(E,V).", interval=0):
            out.append(batch)
            if len(out) == 2:
                break
        return out

    batches = run(collect())
    assert batches[0] == {"added": [["a", 1]], "removed": [], "updated": []}
    assert batches[1] == {"added": [["b", 1]], "removed": [], "updated": [["a", 2]]}


def test_spawn_verb_fact_shape(monkeypatch: pytest.MonkeyPatch) -> None:
    writes: list[Any] = []

    def handler(req: httpx.Request) -> httpx.Response:
        if req.url.path == "/api/query":
            return httpx.Response(200, json={"vars": ["P"], "rows": [[{"t": "str", "v": "prefab:claude-worker"}]], "as_of": 1})
        writes.append(weave.json.loads(req.read()))
        return httpx.Response(200, json=[{"seq": i, "id": f"f{i}"} for i in range(6)])

    install_transport(monkeypatch, handler)
    sid = run(weave.spawn("prefab:claude-worker", "do work", "agent:main"))
    assert sid.startswith("spawn:")
    attrs = [(w["fact"]["attr"], w["fact"]["value"]["v"]) for w in writes[0]]
    assert attrs[:5] == [
        ("type", "spawn_request"),
        ("prefab", "prefab:claude-worker"),
        ("task", "do work"),
        ("requested_by", "agent:main"),
        ("placement", ""),
    ]
    assert attrs[5][0] == "requested_ms"


def test_supervisor_lock_exclusivity(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(supervisor, "runtime_dir", lambda: tmp_path)
    first = supervisor._Lock()
    second = supervisor._Lock()
    assert first.acquire() is True
    assert second.acquire() is False
    first.release()
    assert second.acquire() is True
    second.release()


def test_supervisor_spawn_flow_fake_harness(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    script = tmp_path / "fake-harness"
    script.write_text("#!/bin/sh\necho '{\"ok\":true}'\n")
    script.chmod(0o755)
    monkeypatch.setenv("IX_WEAVE_HARNESS_BIN", str(script))
    answers = {
        "prefab": "prefab:claude-worker",
        "task": "summarize this task",
        "requested_by": "agent:main",
        "harness": "claude-code",
    }

    async def fake_one(program: str) -> Any:
        for key, value in answers.items():
            if key in program:
                return value
        return None

    class FakeClient:
        def __init__(self) -> None:
            self.writes: list[list[tuple[str, str, Any]]] = []

        async def assert_facts(self, facts: list[tuple[str, str, Any]]) -> list[dict[str, Any]]:
            self.writes.append(facts)
            return []

    client = FakeClient()
    monkeypatch.setattr(supervisor, "_one", fake_one)
    run(supervisor._spawn_request(client, "host:test", "spawn:1", "prefab:claude-worker", asyncio.Semaphore(1)))
    first, last = client.writes[0], client.writes[-1]
    assert ("spawn:1", "status", "fulfilled") in first
    assert any(attr == "pid" and isinstance(value, int) for _, attr, value in first)
    assert any(attr == "fulfills" and value == "spawn:1" for _, attr, value in first)
    assert any(attr == "status" and value == "done" for _, attr, value in last)
    assert any(attr == "last_output" and "ok" in value for _, attr, value in last)
