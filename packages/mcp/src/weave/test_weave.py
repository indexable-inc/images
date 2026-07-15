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
