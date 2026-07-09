from __future__ import annotations

import asyncio
import os
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
from weave import supervisor

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


class FakeWeaveClient:
    """Latest-wins in-memory double for the supervisor's client surface.

    Mirrors the weave_stub.py idea: not a datalog engine, just enough journal
    semantics (latest per (entity, attr), retraction by fact id) for the claim
    protocol. ``race_claimed_by`` emulates a rival supervisor whose claim
    lands right after ours in the journal, so latest-wins picks the rival.
    """

    def __init__(
        self,
        latest: dict[tuple[str, str], Any] | None = None,
        race_claimed_by: str | None = None,
    ) -> None:
        self.latest: dict[tuple[str, str], Any] = dict(latest or {})
        self.writes: list[list[tuple[str, str, Any]]] = []
        self.retracted: list[str] = []
        self._facts: dict[str, tuple[str, str, Any]] = {}
        self._seq = 0
        self._race = race_claimed_by

    async def assert_facts(self, facts: list[tuple[str, str, Any]]) -> list[dict[str, Any]]:
        batch = list(facts)
        self.writes.append(batch)
        acks: list[dict[str, Any]] = []
        for entity, attr, value in batch:
            self._seq += 1
            fact_id = f"f{self._seq}"
            self._facts[fact_id] = (entity, attr, value)
            self.latest[(entity, attr)] = value
            if attr == "claimed_by" and self._race is not None:
                self.latest[(entity, attr)] = self._race
            acks.append({"seq": self._seq, "id": fact_id})
        return acks

    async def retract(self, fact_id: str) -> dict[str, Any]:
        self.retracted.append(fact_id)
        entity, attr, value = self._facts[fact_id]
        if self.latest.get((entity, attr)) == value:
            del self.latest[(entity, attr)]
        return {"seq": self._seq, "id": fact_id}


class FakeProc:
    pid = 4242
    returncode = 0

    async def communicate(self) -> tuple[bytes, bytes]:
        return (b'{"ok":true}', b"")

    def kill(self) -> None:
        pass


def supervisor_one(client: FakeWeaveClient, answers: dict[str, object]) -> Callable[[str], Coroutine[Any, Any, object | None]]:
    """A supervisor._one double: claimed_by reads the fake journal, rest is canned."""

    async def fake_one(program: str) -> object | None:
        m = re.match(r"\?- latest\((\S+), claimed_by, \w+\)\.", program)
        if m:
            return client.latest.get((m.group(1), "claimed_by"))
        for key, value in answers.items():
            if key in program:
                return value
        return None

    return fake_one


def test_supervisor_spawn_flow_fake_harness(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    script = tmp_path / "fake-harness"
    script.write_text("#!/bin/sh\necho '{\"ok\":true}'\n")
    script.chmod(0o755)
    monkeypatch.setenv("IX_WEAVE_HARNESS_BIN", str(script))
    answers = {
        "task": "summarize this task",
        "requested_by": "agent:main",
        "harness": "claude-code",
    }
    client = FakeWeaveClient()
    monkeypatch.setattr(supervisor, "_one", supervisor_one(client, answers))
    launched = run(supervisor._spawn_request(client, "host:test", "spawn:1", "prefab:claude-worker", asyncio.Semaphore(1)))
    assert launched is True
    claim, first, last = client.writes[0], client.writes[1], client.writes[-1]
    # The claim is the very first write, in contract order.
    assert [(entity, attr) for entity, attr, _ in claim] == [("spawn:1", "claimed_by"), ("spawn:1", "claimed_ms")]
    assert claim[0][2] == "host:test"
    assert isinstance(claim[1][2], int)
    assert ("spawn:1", "status", "fulfilled") in first
    assert any(attr == "pid" and isinstance(value, int) for _, attr, value in first)
    assert any(attr == "fulfills" and value == "spawn:1" for _, attr, value in first)
    assert any(attr == "status" and value == "done" for _, attr, value in last)
    assert any(attr == "last_output" and "ok" in value for _, attr, value in last)
    assert client.retracted == []


def test_supervisor_claim_asserted_before_launch(monkeypatch: pytest.MonkeyPatch) -> None:
    events: list[tuple[str, list[Any]]] = []
    client = FakeWeaveClient()
    real_assert = client.assert_facts

    async def logged_assert(facts: list[tuple[str, str, Any]]) -> list[dict[str, Any]]:
        events.append(("facts", [attr for _, attr, _ in facts]))
        return await real_assert(facts)

    client.assert_facts = logged_assert  # type: ignore[method-assign]

    async def fake_exec(*argv: str, **kwargs: Any) -> FakeProc:
        events.append(("launch", list(argv)))
        return FakeProc()

    monkeypatch.setattr(supervisor.asyncio, "create_subprocess_exec", fake_exec)
    monkeypatch.setattr(supervisor, "_one", supervisor_one(client, {"task": "do work"}))
    assert run(supervisor._spawn_request(client, "host:test", "spawn:1", "prefab:claude-worker", asyncio.Semaphore(1))) is True
    # Crash-safety: the claim lands before the subprocess exists, and the
    # fulfills/agent facts land only after a successful launch.
    assert events[0] == ("facts", ["claimed_by", "claimed_ms"])
    launch_at = next(i for i, (kind, _) in enumerate(events) if kind == "launch")
    fulfills_at = next(i for i, (kind, payload) in enumerate(events) if kind == "facts" and "fulfills" in payload)
    assert 0 < launch_at < fulfills_at


def test_supervisor_reclaims_after_claimant_crash(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    script = tmp_path / "fake-harness"
    script.write_text("#!/bin/sh\necho '{\"ok\":true}'\n")
    script.chmod(0o755)
    monkeypatch.setenv("IX_WEAVE_HARNESS_BIN", str(script))
    # Journal state a crashed supervisor leaves behind: a claim, no fulfills.
    # Once host:dead's heartbeat is stale the prelude re-derives the request
    # open (pinned in crates/store/tests/weave2_rules.rs), so a fresh
    # supervisor sees it via watch; it must reclaim and launch, not skip.
    client = FakeWeaveClient(
        latest={
            ("spawn:9", "claimed_by"): "host:dead",
            ("spawn:9", "claimed_ms"): 1_000_000,
        }
    )
    monkeypatch.setattr(supervisor, "_one", supervisor_one(client, {"task": "recover"}))
    launched = run(supervisor._spawn_request(client, "host:fresh", "spawn:9", "prefab:claude-worker", asyncio.Semaphore(1)))
    assert launched is True
    # The fresh claim supersedes the dead host's: latest-wins by journal order.
    assert client.writes[0][0] == ("spawn:9", "claimed_by", "host:fresh")
    assert client.latest[("spawn:9", "claimed_by")] == "host:fresh"
    assert any(attr == "fulfills" and value == "spawn:9" for _, attr, value in client.writes[1])
    assert client.retracted == []


def test_supervisor_loser_backs_off_and_retracts(monkeypatch: pytest.MonkeyPatch) -> None:
    # A rival's claim lands right after ours in the journal, so the read-back
    # says the rival owns the request: no launch, no agent facts, and our
    # superseded claim is retracted so it cannot resurrect if the rival
    # later retracts theirs.
    client = FakeWeaveClient(race_claimed_by="host:rival")
    launches: list[tuple[str, ...]] = []

    async def fake_exec(*argv: str, **kwargs: Any) -> FakeProc:
        launches.append(argv)
        return FakeProc()

    monkeypatch.setattr(supervisor.asyncio, "create_subprocess_exec", fake_exec)
    monkeypatch.setattr(supervisor, "_one", supervisor_one(client, {"task": "contested"}))
    launched = run(supervisor._spawn_request(client, "host:loser", "spawn:2", "prefab:claude-worker", asyncio.Semaphore(1)))
    assert launched is False
    assert launches == []
    assert client.writes == [[("spawn:2", "claimed_by", "host:loser"), ("spawn:2", "claimed_ms", client.writes[0][1][2])]]
    assert client.retracted == ["f1", "f2"]
    assert client.latest[("spawn:2", "claimed_by")] == "host:rival"


def test_supervisor_launch_failure_retracts_claim(monkeypatch: pytest.MonkeyPatch) -> None:
    # Launch failure must reopen the request immediately (claim retracted),
    # not strand it until this host's heartbeat goes stale.
    client = FakeWeaveClient()

    async def failing_exec(*argv: str, **kwargs: Any) -> FakeProc:
        raise FileNotFoundError("no harness binary")

    monkeypatch.setattr(supervisor.asyncio, "create_subprocess_exec", failing_exec)
    monkeypatch.setattr(supervisor, "_one", supervisor_one(client, {"task": "doomed"}))
    with pytest.raises(FileNotFoundError):
        run(supervisor._spawn_request(client, "host:test", "spawn:3", "prefab:claude-worker", asyncio.Semaphore(1)))
    assert client.retracted == ["f1", "f2"]
    assert ("spawn:3", "claimed_by") not in client.latest
    assert ("spawn:3", "claimed_ms") not in client.latest
    assert all(attr != "fulfills" for batch in client.writes for _, attr, _ in batch)
