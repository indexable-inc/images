from __future__ import annotations

import asyncio
import hashlib
import json
import re
import sys
import threading
from collections.abc import AsyncIterator, Coroutine
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, TypeVar

import httpx
import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "weave"))
sys.path.insert(0, str(ROOT / "fabric"))

import weave
from claude_agent_sdk import AssistantMessage, Message, ResultMessage, TextBlock

import fabric
from fabric import activity, claude, reconcile, remote

_T = TypeVar("_T")

_HEX64 = re.compile(r"^[0-9a-f]{64}$")


def run(coro: Coroutine[Any, Any, _T]) -> _T:
    return asyncio.run(coro)


@dataclass
class Journal:
    """In-memory weave double behind an ``httpx.MockTransport``."""

    facts: list[tuple[object, str, object]] = field(default_factory=list)
    blobs: dict[str, bytes] = field(default_factory=dict)
    interrupt: str | None = None
    #: canned response per exact datalog program (reconcile/activity queries);
    #: anything else is answered as a per-entity interrupt watch.
    queries: dict[str, dict[str, Any]] = field(default_factory=dict)

    def handler(self, req: httpx.Request) -> httpx.Response:
        path = req.url.path
        if path == "/api/blob":
            body = req.read()
            # Any 64-hex digest satisfies the client's hash contract.
            digest = hashlib.sha256(body).hexdigest()
            self.blobs[digest] = body
            return httpx.Response(200, json={"hash": digest})
        if path == "/api/facts":
            payload = json.loads(req.read())
            batch = payload if isinstance(payload, list) else [payload]
            for item in batch:
                fact = item["fact"]
                self.facts.append((fact["entity"]["v"], fact["attr"], fact["value"]["v"]))
            return httpx.Response(200, json=[{"seq": i, "id": f"f{i}"} for i in range(len(batch))])
        if path == "/api/query":
            program = json.loads(req.read())["program"]
            if program in self.queries:
                return httpx.Response(200, json=self.queries[program])
            rows = [] if self.interrupt is None else [[{"t": "str", "v": self.interrupt}]]
            return httpx.Response(200, json={"vars": ["I"], "rows": rows, "as_of": 1})
        raise AssertionError(f"unexpected weave call: {path}")

    def states(self, task: str) -> list[object]:
        return [v for e, a, v in self.facts if e == task and a == "state"]

    def blob_for(self, task: str, attr: str) -> bytes:
        values = [v for e, a, v in self.facts if e == task and a == attr]
        assert values, f"no {attr} fact on {task}"
        digest = values[-1]
        assert isinstance(digest, str), digest
        assert _HEX64.fullmatch(digest), digest
        return self.blobs[digest]


def install(monkeypatch: pytest.MonkeyPatch) -> Journal:
    journal = Journal()
    transport = httpx.MockTransport(journal.handler)
    monkeypatch.setattr(
        weave,
        "_client",
        lambda **kw: httpx.AsyncClient(transport=transport, base_url="http://weave.test", **kw),
    )
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:tester")
    return journal


class FakeClient:
    """Structural stand-in for ``ClaudeSDKClient`` (see ``claude.SdkClient``)."""

    def __init__(self, *, connect_error: Exception | None = None) -> None:
        self.connected = False
        self.interrupts = 0
        self.queries: list[str] = []
        self._connect_error = connect_error
        self._messages: asyncio.Queue[Message | None] = asyncio.Queue()

    def feed(self, message: Message) -> None:
        self._messages.put_nowait(message)

    async def connect(self, prompt: str | None = None) -> None:
        if self._connect_error is not None:
            raise self._connect_error
        self.connected = True

    async def query(self, prompt: str, session_id: str = "default") -> None:
        self.queries.append(prompt)

    async def receive_messages(self) -> AsyncIterator[Message]:
        while True:
            message = await self._messages.get()
            if message is None:
                return
            yield message

    async def interrupt(self) -> None:
        self.interrupts += 1

    async def disconnect(self) -> None:
        self.connected = False


def use_fake(monkeypatch: pytest.MonkeyPatch, fake: FakeClient) -> None:
    monkeypatch.setattr(claude, "_sdk_client", lambda **kw: fake)
    monkeypatch.setattr(fabric, "INTERRUPT_POLL_S", 0.01)


def result_message(text: str) -> ResultMessage:
    return ResultMessage(
        subtype="success",
        duration_ms=1,
        duration_api_ms=1,
        is_error=False,
        num_turns=1,
        session_id="s1",
        result=text,
    )


# --- fabric.run ---------------------------------------------------------------


def test_run_records_ask_then_started_then_done(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)

    def add(a: int, b: int) -> int:
        return a + b

    async def main() -> tuple[fabric.RunHandle, object]:
        handle = await fabric.run(add, 2, 3)
        return handle, await handle.wait()

    handle, value = run(main())
    assert value == 5
    assert re.fullmatch(r"task:[0-9a-f]{8}", handle.task)
    attrs = [(a, v) for e, a, v in journal.facts if e == handle.task]
    # Ask facts land at submit, state strictly last; the worker wrapper then
    # appends started (running) and the terminal state, again last.
    assert [a for a, _ in attrs] == [
        "type",
        "fn",
        "node",
        "requested_by",
        "source",
        "state",
        "state",
        "result",
        "state",
    ]
    assert dict(attrs)["requested_by"] == "agent:tester"
    assert dict(attrs)["fn"].endswith("add")
    assert journal.states(handle.task) == ["submitted", "running", "done"]
    assert b"def add(a: int, b: int) -> int:" in journal.blob_for(handle.task, "source")
    assert journal.blob_for(handle.task, "result") == b"5"


def test_run_sync_off_loop_and_async_native(monkeypatch: pytest.MonkeyPatch) -> None:
    install(monkeypatch)

    def thread_name() -> str:
        return threading.current_thread().name

    async def double(n: int) -> int:
        return n * 2

    async def main() -> tuple[object, object]:
        sync_result = await (await fabric.run(thread_name))
        async_result = await (await fabric.run(double, 21))
        return sync_result, async_result

    sync_result, async_result = run(main())
    assert sync_result != "MainThread"  # sync fns run in to_thread, off the loop
    assert async_result == 42


@pytest.mark.parametrize(
    ("args", "detail"),
    [
        ((), "RuntimeError: nope"),  # raises on its first line
        ((1, 2), "TypeError"),  # raises before its first line: bad signature bind
    ],
)
def test_run_failure_still_leaves_ask_and_failed(
    monkeypatch: pytest.MonkeyPatch, args: tuple[int, ...], detail: str
) -> None:
    journal = install(monkeypatch)

    def boom() -> None:
        raise RuntimeError("nope")

    async def main() -> fabric.RunHandle:
        handle = await fabric.run(boom, *args)
        with pytest.raises((RuntimeError, TypeError)):
            await handle.wait()
        return handle

    handle = run(main())
    assert journal.states(handle.task) == ["submitted", "running", "failed"]
    errors = [v for e, a, v in journal.facts if e == handle.task and a == "error"]
    assert len(errors) == 1
    assert str(errors[0]).startswith(detail)
    # The terminal state is the entity's last fact.
    assert journal.facts[-1] == (handle.task, "state", "failed")
    assert b"def boom() -> None:" in journal.blob_for(handle.task, "source")


@pytest.mark.parametrize(
    ("kwargs", "match"),
    [
        ({"local": False}, "requires node="),
        ({"node": "hc1", "local": True}, "contradicts"),
        ({"cpus": 2.0}, "remote placement"),
        ({"node": "hc1", "repo": "r"}, "come together"),
    ],
)
def test_run_rejects_contradictory_placement(
    monkeypatch: pytest.MonkeyPatch, kwargs: dict[str, object], match: str
) -> None:
    journal = install(monkeypatch)

    async def main() -> None:
        await fabric.run(int, **kwargs)

    with pytest.raises(ValueError, match=match):
        run(main())
    assert journal.facts == []  # rejected before any journal write


def test_run_remote_records_target_node_and_done(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    placement = remote.Placement(node="hc1", label="host_hc1")
    shipped: list[tuple[object, ...]] = []

    async def fake_prepare(node: str) -> remote.Placement:
        assert node == "hc1"
        return placement

    async def fake_execute(*args: object, **kwargs: object) -> object:
        shipped.append(args)
        return 7

    monkeypatch.setattr(remote, "prepare", fake_prepare)
    monkeypatch.setattr(remote, "execute", fake_execute)

    def seven() -> int:
        return 7

    async def main() -> tuple[fabric.RunHandle, object]:
        handle = await fabric.run(seven, node="hc1")
        return handle, await handle.wait()

    handle, value = run(main())
    assert value == 7
    assert shipped
    assert shipped[0][0] is placement
    # The ask's node fact names the TARGET, not the submitting host, and the
    # runner fact names the actor the reconciler diffs against live actors.
    assert (handle.task, "node", "hc1") in journal.facts
    assert (handle.task, "runner", "runner:hc1") in journal.facts
    assert journal.states(handle.task) == ["submitted", "running", "done"]


def test_run_remote_bad_target_raises_before_facts(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)

    async def fake_prepare(node: str) -> remote.Placement:
        raise remote.FabricError(f"no live Ray node advertises 'host_{node}'")

    monkeypatch.setattr(remote, "prepare", fake_prepare)

    async def main() -> None:
        await fabric.run(int, node="ghost")

    with pytest.raises(remote.FabricError, match="host_ghost"):
        run(main())
    assert journal.facts == []  # a bad target fails the run() call itself


# --- fabric.remote submit-time checks ------------------------------------------


def test_local_env_missing_raises(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(remote.ENV_VAR, raising=False)
    with pytest.raises(remote.FabricError, match="IX_FABRIC_ENV"):
        remote.local_env()


def test_check_host_label_names_known_hosts() -> None:
    resources = {"host_hc1": 1.0, "host_hydra": 1.0, "CPU": 8.0}
    assert remote.check_host_label("hc1", resources) == "host_hc1"
    with pytest.raises(remote.FabricError, match=r"host_ghost.*hc1.*hydra"):
        remote.check_host_label("ghost", resources)


def test_check_env_skew_names_both_sides() -> None:
    local = "fabric_env:py3.13-ray2.56.0"
    remote.check_env("hc1", {local: 1.0, "CPU": 8.0}, local)
    with pytest.raises(remote.EnvSkewError, match=r"py3\.13-ray2\.56\.0.*py3\.12-ray2\.44\.0"):
        remote.check_env("hc1", {"fabric_env:py3.12-ray2.44.0": 1.0}, local)
    with pytest.raises(remote.EnvSkewError, match="no fabric_env resource"):
        remote.check_env("hc1", {"CPU": 8.0}, local)


def test_zero_restart_policy_everywhere() -> None:
    # Policy, not a default: no caller input reaches these, so this is the
    # one place a restarted-runner path could sneak back in.
    actor = remote.actor_options("hc1")
    assert actor["max_restarts"] == 0
    assert actor["lifetime"] == "detached"
    assert actor["name"] == "runner:hc1"
    assert remote.task_options("hc1", cpus=4)["max_retries"] == 0


def test_runner_payload_roundtrip_through_ray_cloudpickle() -> None:
    from ray import cloudpickle

    offset = 100

    def work(a: int, b: int) -> int:  # a closure: travels by value
        return a + b + offset

    payload = cloudpickle.dumps(
        remote._Task(fn=work, args=(2, 3), kwargs={}, workspace=None)
    )
    out = run(remote.Runner().run(payload))
    assert cloudpickle.loads(out) == 105


def _git_fixture(root: Path) -> tuple[str, str]:
    """A two-commit repo; returns (repo path, first rev)."""
    import subprocess

    repo = root / "fixture"
    repo.mkdir()
    env_git = ["git", "-C", str(repo), "-c", "user.name=t", "-c", "user.email=t@t"]
    subprocess.run([*env_git[:3], "init", "-q", "-b", "main"], check=True)
    (repo / "data.txt").write_text("one")
    subprocess.run([*env_git, "add", "data.txt"], check=True)
    subprocess.run([*env_git, "commit", "-qm", "one"], check=True)
    first = subprocess.run(
        [*env_git[:3], "rev-parse", "HEAD"], check=True, capture_output=True, text=True
    ).stdout.strip()
    (repo / "data.txt").write_text("two")
    subprocess.run([*env_git, "commit", "-qam", "two"], check=True)
    return str(repo), first


def test_workspace_materializes_exact_rev(tmp_path: Path) -> None:
    repo, first = _git_fixture(tmp_path)
    checkout = remote.materialize(remote.Workspace(repo=repo, rev=first))
    assert (checkout / "data.txt").read_text() == "one"  # the pinned rev, not HEAD
    with pytest.raises(remote.FabricError, match="checkout"):
        remote.materialize(remote.Workspace(repo=repo, rev="0" * 40))


def test_runner_hands_workspace_path_and_cleans_up(tmp_path: Path) -> None:
    from ray import cloudpickle

    repo, first = _git_fixture(tmp_path)

    # cloudpickle ships closures by value, so report the workdir through the
    # return value rather than a captured list.
    def read(workdir: Path) -> tuple[str, str]:
        return str(workdir), (workdir / "data.txt").read_text()

    payload = cloudpickle.dumps(
        remote._Task(fn=read, args=(), kwargs={}, workspace=remote.Workspace(repo=repo, rev=first))
    )
    out = run(remote.Runner().run(payload))
    workdir, content = cloudpickle.loads(out)
    assert content == "one"
    assert not Path(workdir).exists()  # per-run scratch removed after the run


def test_run_interrupt_records_interrupted(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)

    async def forever() -> None:
        await asyncio.sleep(60)

    async def main() -> fabric.RunHandle:
        handle = await fabric.run(forever)
        await asyncio.sleep(0)  # let the worker publish running
        await handle.interrupt()
        with pytest.raises(asyncio.CancelledError):
            await handle.wait()
        return handle

    handle = run(main())
    assert journal.states(handle.task) == ["submitted", "running", "interrupted"]
    assert (handle.task, "interrupt", "requested") in journal.facts


def test_run_interrupt_fact_path(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    monkeypatch.setattr(fabric, "INTERRUPT_POLL_S", 0.01)

    async def forever() -> None:
        await asyncio.sleep(60)

    async def main() -> fabric.RunHandle:
        handle = await fabric.run(forever)
        await asyncio.sleep(0)  # let the worker publish running
        # Someone else asserts interrupt=requested on the run entity; the
        # run's own watcher routes it into asyncio cancellation (no
        # dispatcher loop anywhere).
        journal.interrupt = "requested"
        with pytest.raises(asyncio.CancelledError):
            async with asyncio.timeout(5):
                await handle.wait()
        return handle

    handle = run(main())
    assert journal.states(handle.task) == ["submitted", "running", "interrupted"]


# --- claude.session -----------------------------------------------------------


def test_session_records_turns_result_and_done(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    fake = FakeClient()
    use_fake(monkeypatch, fake)

    async def main() -> claude.Session:
        live = await claude.session("solve the riddle", model="opus")
        assert fake.connected
        assert fake.queries == ["solve the riddle"]
        fake.feed(AssistantMessage(content=[TextBlock(text="thinking")], model="opus"))
        fake.feed(result_message("the answer"))
        assert await live.result(timeout=5) == "the answer"
        await live.close()
        return live

    live = run(main())
    task = live.task
    assert journal.states(task) == ["submitted", "running", "done"]
    assert journal.facts[-1] == (task, "state", "done")
    # Payloads live in CAS; facts carry only pointers.
    assert journal.blob_for(task, "prompt") == b"solve the riddle"
    assert journal.blob_for(task, "result") == b"the answer"
    for _, _attr, value in [f for f in journal.facts if f[0] == task]:
        assert "the answer" not in str(value)
        assert "solve the riddle" not in str(value)
    turns = [v for e, a, v in journal.facts if e == task and a == "turn"]
    assert len(turns) == 2
    first = json.loads(journal.blobs[str(turns[0])])
    assert first["type"] == "AssistantMessage"
    assert first["message"]["content"] == [{"text": "thinking"}]
    assert json.loads(journal.blobs[str(turns[1])])["type"] == "ResultMessage"


def test_session_follow_up_input_streams(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    fake = FakeClient()
    use_fake(monkeypatch, fake)

    async def main() -> claude.Session:
        live = await claude.session("first")
        fake.feed(result_message("one"))
        assert await live.result(timeout=5) == "one"
        await live.send("second")
        fake.feed(result_message("two"))
        assert await live.result(timeout=5) == "two"
        await live.close()
        return live

    live = run(main())
    assert fake.queries == ["first", "second"]
    turns = [v for e, a, v in journal.facts if e == live.task and a == "turn"]
    assert len(turns) == 3  # result one, the follow-up user turn, result two
    follow_up = json.loads(journal.blobs[str(turns[1])])
    assert follow_up == {"type": "UserMessage", "message": {"content": "second"}}


def test_interrupt_handle_path(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    fake = FakeClient()
    use_fake(monkeypatch, fake)

    async def main() -> claude.Session:
        live = await claude.session("long job")
        await live.interrupt()
        await live.close()  # must not overwrite the terminal state
        return live

    live = run(main())
    assert fake.interrupts == 1  # converged on the SDK interrupt
    assert journal.states(live.task) == ["submitted", "running", "interrupted"]
    assert (live.task, "interrupt", "requested") in journal.facts


def test_interrupt_fact_path(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    fake = FakeClient()
    use_fake(monkeypatch, fake)

    async def main() -> claude.Session:
        live = await claude.session("long job")
        # Someone else asserts interrupt=requested on the run entity.
        journal.interrupt = "requested"
        await live.result(timeout=5)  # released by the interrupt
        await live.close()
        return live

    live = run(main())
    assert fake.interrupts == 1  # the journal watcher converged on the SDK interrupt
    assert journal.states(live.task) == ["submitted", "running", "interrupted"]


def test_session_connect_failure_leaves_ask_and_failed(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    fake = FakeClient(connect_error=OSError("claude CLI missing"))
    use_fake(monkeypatch, fake)

    async def main() -> None:
        await claude.session("never starts")

    with pytest.raises(OSError, match="claude CLI missing"):
        run(main())
    tasks = {e for e, a, v in journal.facts if a == "type"}
    assert len(tasks) == 1
    task = tasks.pop()
    assert isinstance(task, str)
    assert journal.states(task) == ["submitted", "failed"]
    assert journal.blob_for(task, "prompt") == b"never starts"
    assert journal.facts[-1] == (task, "state", "failed")


# --- fabric.reconcile -----------------------------------------------------------


def _wrap_rows(rows: list[list[object]]) -> dict[str, Any]:
    def wrap(v: object) -> dict[str, object]:
        return {"t": "int", "v": v} if isinstance(v, int) else {"t": "str", "v": str(v)}

    return {"vars": [], "rows": [[wrap(v) for v in row] for row in rows], "as_of": 1}


def test_reconcile_parse_grace_and_open_states() -> None:
    rows = [
        # Same state fact twice: the newest write time wins, and 70s old is
        # past the 60s grace window.
        ["task:aaaa0001", "runner:hc1", "running", "f1", 0],
        ["task:aaaa0001", "runner:hc1", "running", "f2", 30_000],
        # Terminal: settled runs are never reconciled.
        ["task:aaaa0002", "runner:hc1", "done", "f3", 0],
        # Open but inside the grace window: Ray may not have created the
        # actor yet, so hands off.
        ["task:aaaa0003", "runner:hc2", "submitted", "f4", 99_000],
    ]
    assert reconcile._parse(rows, now_ms=100_000) == [("task:aaaa0001", "runner:hc1")]


def test_reconcile_once_marks_dead_runner_lost(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    journal.queries[reconcile.QUERY] = _wrap_rows([
        ["task:aaaa0001", "runner:hc1", "running", "f1", 0],
        ["task:aaaa0002", "runner:hc2", "running", "f2", 0],
        ["task:aaaa0003", "runner:hc1", "done", "f3", 0],
    ])
    probed: list[set[str]] = []

    def fake_alive(candidates: set[str]) -> set[str]:
        probed.append(set(candidates))
        return {"runner:hc2"}

    monkeypatch.setattr(reconcile, "_alive_runners", fake_alive)

    assert run(reconcile.once()) == ["task:aaaa0001"]
    # Only the open runs' runners are probed; the settled run is not.
    assert probed == [{"runner:hc1", "runner:hc2"}]
    # The lost record: an error fact naming the dead runner, state strictly
    # last. It NEVER restarts: the only journal write is the terminal fact.
    assert journal.facts == [
        ("task:aaaa0001", "error", "reconciler: runner:hc1 died without a terminal fact"),
        ("task:aaaa0001", "state", "lost"),
    ]


def test_reconcile_nothing_stale_skips_ray(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    journal.queries[reconcile.QUERY] = _wrap_rows([
        ["task:aaaa0001", "runner:hc1", "done", "f1", 0],
    ])

    def boom(candidates: set[str]) -> set[str]:
        raise AssertionError("no open runs: the cluster must not be probed")

    monkeypatch.setattr(reconcile, "_alive_runners", boom)
    assert run(reconcile.once()) == []
    assert journal.facts == []


# --- fabric.activity ------------------------------------------------------------


def test_activity_frame_is_the_per_node_view(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)
    journal.queries[activity.QUERY] = _wrap_rows([
        ["task:aaaa0001", "hydra", "build_index", "running"],
        ["task:aaaa0002", "hc1", "scrape", "done"],
        ["task:aaaa0003", "hc1", "scrape", "submitted"],
        ["task:aaaa0004", "hc2", "train", "lost"],
    ])

    open_df = run(activity.frame())
    assert open_df.columns == ["task", "node", "fn", "state"]
    assert open_df.rows() == [
        ("task:aaaa0003", "hc1", "scrape", "submitted"),
        ("task:aaaa0001", "hydra", "build_index", "running"),
    ]
    history = run(activity.frame(open_only=False))
    assert history.height == 4
