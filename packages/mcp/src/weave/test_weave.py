from __future__ import annotations

import asyncio
import re
import sys
from collections.abc import Callable, Coroutine
from pathlib import Path
from typing import Any, TypeVar

import httpx
import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "weave"))
sys.path.insert(0, str(ROOT.parent))

import weave

_T = TypeVar("_T")


def run(coro: Coroutine[Any, Any, _T]) -> _T:
    return asyncio.run(coro)


def install_transport(monkeypatch: pytest.MonkeyPatch, handler: Callable[[httpx.Request], httpx.Response]) -> list[Any]:
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
    run(weave.assert_fact("e", "b", True))  # noqa: FBT003 - the bool VALUE mapping is the subject under test
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


def test_result_returns_on_done(monkeypatch: pytest.MonkeyPatch) -> None:
    states = iter(["pending", "done"])
    programs: list[str] = []

    def handler(req: httpx.Request) -> httpx.Response:
        program = weave.json.loads(req.read())["program"]
        programs.append(program)
        if ", state, " in program:
            return httpx.Response(200, json={"vars": ["S"], "rows": [[{"t": "str", "v": next(states)}]], "as_of": 1})
        return httpx.Response(200, json={"vars": ["R"], "rows": [[{"t": "str", "v": "all done here"}]], "as_of": 2})

    install_transport(monkeypatch, handler)
    assert run(weave.result("task-abcd1234")) == "all done here"
    # One state poll per 0.5s tick until terminal, then one result read.
    assert programs == [
        '?- latest("task-abcd1234", state, S).',
        '?- latest("task-abcd1234", state, S).',
        '?- latest("task-abcd1234", result, R).',
    ]


@pytest.mark.parametrize(
    ("state", "attr", "detail", "error"),
    [
        ("failed", "error", "worker process exited", weave.TaskFailedError),
        ("lost", "error", "reconciler: runner:hc1 died without a terminal fact", weave.TaskFailedError),
        ("cancelled", "result", "stopped by user", weave.TaskCancelledError),
        ("interrupted", "result", "stopped via interrupt fact", weave.TaskCancelledError),
    ],
)
def test_result_raises_terminal_failure(
    monkeypatch: pytest.MonkeyPatch,
    state: str,
    attr: str,
    detail: str,
    error: type[RuntimeError],
) -> None:
    programs: list[str] = []

    def handler(req: httpx.Request) -> httpx.Response:
        program = weave.json.loads(req.read())["program"]
        programs.append(program)
        value = state if ", state, " in program else detail
        variable = "S" if ", state, " in program else "R"
        return httpx.Response(200, json={"vars": [variable], "rows": [[{"t": "str", "v": value}]], "as_of": 1})

    install_transport(monkeypatch, handler)
    with pytest.raises(error, match=rf"task {state}: task-abcd1234: {detail}"):
        run(weave.result("task-abcd1234"))
    assert programs == [
        '?- latest("task-abcd1234", state, S).',
        f'?- latest("task-abcd1234", {attr}, R).',
    ]


def test_result_times_out(monkeypatch: pytest.MonkeyPatch) -> None:
    def handler(req: httpx.Request) -> httpx.Response:
        return httpx.Response(200, json={"vars": ["S"], "rows": [[{"t": "str", "v": "pending"}]], "as_of": 1})

    install_transport(monkeypatch, handler)
    with pytest.raises(TimeoutError):
        run(weave.result("task-abcd1234", timeout=0))


# --- record / spool (durable-local-first, index#3418) ---------------------------


def _journal_transport(monkeypatch: pytest.MonkeyPatch, *, down: dict[str, bool]) -> tuple[list[tuple[str, str, Any]], dict[str, bytes]]:
    """A weave double that can be toggled unreachable via ``down['is']``."""
    import hashlib
    import json as jsonlib

    facts: list[tuple[str, str, Any]] = []
    blobs: dict[str, bytes] = {}

    def handler(req: httpx.Request) -> httpx.Response:
        if down["is"]:
            raise httpx.ConnectError("connection refused")
        if req.url.path == "/api/blob":
            body = req.read()
            digest = hashlib.sha256(body).hexdigest()
            blobs[digest] = body
            return httpx.Response(200, json={"hash": digest})
        assert req.url.path == "/api/facts"
        for item in jsonlib.loads(req.read()):
            fact = item["fact"]
            facts.append((fact["entity"]["v"], fact["attr"], fact["value"]["v"]))
        return httpx.Response(200, json=[])

    install_transport(monkeypatch, handler)
    return facts, blobs


def test_record_is_durable_before_delivery_and_drains_in_order(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    down = {"is": True}
    facts, blobs = _journal_transport(monkeypatch, down=down)

    run(weave.record([("task:1", "state", "submitted"), ("task:1", "prompt", weave.Blob(b"do it"))]))
    run(weave.record([("task:1", "state", "running")]))

    # Durable on disk while the server is unreachable; nothing delivered.
    segments = list((tmp_path / "spool").glob("*.jsonl"))
    assert len(segments) == 1
    lines = [weave.json.loads(line) for line in segments[0].read_text().splitlines()]
    assert [item.get("fact", {}).get("attr", "blob") for item in lines] == ["state", "blob", "state"]
    assert facts == []

    down["is"] = False
    assert run(weave.flush(timeout=10))
    # Delivered in local append order, the blob resolved to a hash-valued fact.
    assert [(e, a) for e, a, _v in facts] == [
        ("task:1", "state"),
        ("task:1", "prompt"),
        ("task:1", "state"),
    ]
    assert facts[0][2] == "submitted"
    assert facts[2][2] == "running"
    assert blobs[facts[1][2]] == b"do it"


def test_spool_transition_prints_exactly_one_loud_line(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    down = {"is": True}
    _journal_transport(monkeypatch, down=down)
    for i in range(5):
        run(weave.record([("task:2", "n", i)]))
    assert not run(weave.flush(timeout=1.0))  # unreachable: flush times out, drops nothing
    err = capsys.readouterr().err
    assert err.count("unreachable") == 1
    assert "drain when it returns" in err
    down["is"] = False
    assert run(weave.flush(timeout=10))
    err = capsys.readouterr().err
    assert err.count("reachable again") == 1


def test_spool_orphan_segment_adopted_and_drained(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    down = {"is": False}
    facts, _blobs = _journal_transport(monkeypatch, down=down)
    orphan = tmp_path / "spool"
    orphan.mkdir(parents=True)
    # A crashed writer's segment: two committed lines plus one torn append
    # (never fsync-completed, so never intent) - no live flock holder.
    (orphan / "w-999-dead.jsonl").write_text(
        '{"fact": {"entity": {"t": "str", "v": "task:9"}, "attr": "state", "value": {"t": "str", "v": "submitted"}}}\n'
        '{"fact": {"entity": {"t": "str", "v": "task:9"}, "attr": "state", "value": {"t": "str", "v": "running"}}}\n'
        '{"fact": {"entity": {"t": "str", "v": "task:9"}, "att'
    )
    run(weave.record([("task:10", "state", "submitted")]))
    assert run(weave.flush(timeout=10))
    assert ("task:9", "state", "submitted") in facts
    assert ("task:9", "state", "running") in facts
    assert ("task:10", "state", "submitted") in facts
    # Orphan segments drain before this process's own appends.
    assert facts.index(("task:9", "state", "running")) < facts.index(("task:10", "state", "submitted"))
    assert not (orphan / "w-999-dead.jsonl").exists()  # retired once drained


def test_spool_auth_denial_parks_after_one_attempt(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    attempts: list[str] = []

    def handler(req: httpx.Request) -> httpx.Response:
        attempts.append(req.url.path)
        return httpx.Response(401, json={"error": "no"})

    install_transport(monkeypatch, handler)
    run(weave.record([("task:3", "state", "submitted")]))
    assert run(weave.flush(timeout=10))  # parked is terminal: flush must not wedge
    assert attempts == ["/api/facts"]  # exactly one attempt, no retry loop
    assert "rejected writes permanently" in capsys.readouterr().err
    # The segment is retained on disk for a future (fixed-credential) process.
    sp = weave._default_spool()
    assert sp.parked
    assert not sp._own.drained
