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
from fabric import claude

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
    monkeypatch.setattr(claude, "INTERRUPT_POLL_S", 0.01)


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


def test_run_remote_placement_not_implemented(monkeypatch: pytest.MonkeyPatch) -> None:
    journal = install(monkeypatch)

    async def main() -> None:
        await fabric.run(int, local=False)

    with pytest.raises(NotImplementedError, match="local-only"):
        run(main())
    assert journal.facts == []  # rejected before any journal write


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
