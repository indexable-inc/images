"""``jobs.spawn`` registers an ad-hoc awaitable as a first-class background job
(issue #2164).

The kernel's job lifecycle (dashboard card, completion notification, pageable
``jobs['<id>']`` handle, ``await`` yields the value / re-raises the failure) used
to be reachable only through ``python_exec`` cells; an awaitable an agent created
itself (a coroutine, a Task) had none of it. ``jobs.spawn(coro, name=...)`` gives
any awaitable the same lifecycle: it appears in ``jobs``, its completion sends
the same channel notification a backgrounded cell sends, and its result follows
the existing Job contract (``.result`` raises while running, a failure re-raises
on ``await``, cancel works).
"""

from __future__ import annotations

import asyncio
import sys

import pytest

from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


def test_spawn_registers_and_awaiting_yields_the_value(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})

    async def work() -> str:
        await asyncio.sleep(0)
        return "payload"

    async def drive() -> tuple[runtime.Job, runtime.Result | None]:
        job = runtime.jobs.spawn(work(), name="my-work")
        # First-class from the moment spawn returns: registered and running.
        assert runtime.jobs[job.id] is job
        assert job.name == "my-work"
        assert job.kind == "spawn"
        assert job.backgrounded
        return job, await job

    job, result = asyncio.run(drive())
    assert job.status == "done"
    assert job.ok
    # The value follows the cell contract: wrapped in a Result whose .value is
    # the ORIGINAL object and whose .text is the model-facing rendering.
    assert result.value == "payload"
    assert "payload" in result.text
    # `.result` hands back the same thing once the job is done.
    assert job.result is result


def test_spawn_name_defaults_to_the_coroutine_qualname(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})

    async def named_work() -> None:
        return None

    async def drive() -> runtime.Job:
        job = runtime.jobs.spawn(named_work())
        await job.wait(5)
        return job

    job = asyncio.run(drive())
    assert "named_work" in job.name


def test_spawned_stdout_is_captured_under_the_job(monkeypatch: pytest.MonkeyPatch) -> None:
    # Prints made while the awaitable runs must land in the job's buffer (the
    # pageable `jobs['<id>'].output`), exactly like a cell's prints. Capture
    # rides the same _Tee + _ix_current plumbing install() wires up, so give the
    # bare test process the tee (install() is not run here).
    _wire(monkeypatch, {})
    monkeypatch.setattr(sys, "stdout", runtime._Tee(sys.stdout))

    async def chatty() -> None:
        print("spawned hello")

    async def drive() -> runtime.Job:
        job = runtime.jobs.spawn(chatty(), name="chatty")
        await job
        return job

    job = asyncio.run(drive())
    assert job.status == "done"
    assert "spawned hello" in job.output
    # A None-valued awaitable reports its stdout, like a print-only cell.
    assert "spawned hello" in job.result.text


def test_awaiting_a_failed_spawn_reraises_the_original_exception(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})

    async def boom() -> None:
        raise ValueError("spawn boom")

    async def drive() -> runtime.Job:
        job = runtime.jobs.spawn(boom(), name="boom")
        await job.wait(5)
        return job

    job = asyncio.run(drive())
    assert job.status == "error"
    assert "spawn boom" in (job.error or "")

    async def await_it() -> object:
        return await runtime.jobs[job.id]

    with pytest.raises(ValueError, match="spawn boom"):
        asyncio.run(await_it())


def test_result_while_running_raises_job_still_running(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})

    async def drive() -> None:
        started = asyncio.Event()

        async def hang() -> None:
            started.set()
            await asyncio.sleep(60)

        job = runtime.jobs.spawn(hang(), name="hang")
        await started.wait()
        with pytest.raises(runtime.JobStillRunning):
            _ = job.result
        job.cancel()
        await job.wait(5)
        assert job.status == "cancelled"

    asyncio.run(drive())


def test_cancelling_a_spawned_task_shape_cancels_the_work(monkeypatch: pytest.MonkeyPatch) -> None:
    # A pre-created Task keeps running when only the runner is cancelled; spawn
    # forwards the cancellation so `job.cancel()` stops the work itself.
    _wire(monkeypatch, {})

    async def drive() -> None:
        started = asyncio.Event()

        async def hang() -> None:
            started.set()
            await asyncio.sleep(60)

        inner = asyncio.ensure_future(hang())
        job = runtime.jobs.spawn(inner, name="hang-task")
        await started.wait()
        job.cancel()
        await job.wait(5)
        assert job.status == "cancelled"
        # The forwarded cancellation reaches the inner task itself.
        await asyncio.wait({inner}, timeout=5)
        assert inner.cancelled()

    asyncio.run(drive())


def test_completion_sends_the_backgrounded_job_notification(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})
    sent: list[tuple[str, dict]] = []

    async def fake_notify(content: str, **meta: object) -> None:
        sent.append((content, meta))

    monkeypatch.setattr(runtime, "notify", fake_notify)

    async def work() -> int:
        return 7

    async def drive() -> runtime.Job:
        job = runtime.jobs.spawn(work(), name="notified")
        await job
        return job

    job = asyncio.run(drive())
    assert job.status == "done"
    assert len(sent) == 1
    content, meta = sent[0]
    assert "notified" in content
    assert "done" in content
    assert meta["job_id"] == job.id
    assert meta["status"] == "done"


def test_spawn_rejects_a_non_awaitable(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})

    async def drive() -> None:
        with pytest.raises(TypeError, match="awaitable"):
            runtime.jobs.spawn(42)  # type: ignore[arg-type]  -- the rejection under test

    asyncio.run(drive())


def test_spawn_outside_the_loop_fails_without_registering(monkeypatch: pytest.MonkeyPatch) -> None:
    # Misuse from a sync context must not leave a forever-"running" phantom job.
    _wire(monkeypatch, {})
    before = set(runtime.jobs)

    coro = asyncio.sleep(0)
    with pytest.raises(RuntimeError):
        runtime.jobs.spawn(coro, name="no-loop")
    coro.close()  # the coroutine was never scheduled; close it quietly
    assert set(runtime.jobs) == before


def test_spawned_job_shows_up_in_history(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})

    async def work() -> str:
        return "x"

    async def drive() -> runtime.Job:
        job = runtime.jobs.spawn(work(), name="hist-entry")
        await job
        return job

    job = asyncio.run(drive())
    listing = runtime.history(200).text
    assert job.id in listing
