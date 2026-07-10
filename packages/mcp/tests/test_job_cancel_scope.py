"""One session's wait must never cancel another session's job (issue #2104).

``jobs`` is a single kernel-wide registry, and ``await jobs['<id>']`` used to
await the job's task DIRECTLY. Two cascades followed under multi-agent load:

- an awaiter's own cancellation -- the common shape being an
  ``asyncio.wait_for(jobs['<id>'], t)`` timeout -- propagated into the shared
  task and cancelled the job for every session (production: job b20422b5 died
  exactly 45s after another agent's ``wait_for(j, timeout=45)`` began);
- cancelling a job threw ``CancelledError`` into every other cell awaiting it,
  so those innocent cells were recorded "cancelled" too (production: c772591c
  and c821f156, two agents' waiter cells, both died the instant 61bd699b was
  explicitly cancelled).

Awaiting now shields the job's task: a wait timing out (or the awaiting cell
being cancelled) leaves the job running, and a cancelled job surfaces
``JobCancelled`` -- an ordinary error naming the job -- in its awaiters.
Explicit ``jobs['<id>'].cancel()`` remains the one way to cancel a job.
"""

from __future__ import annotations

import asyncio

import pytest

from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


def test_wait_for_timeout_leaves_the_job_running(monkeypatch: pytest.MonkeyPatch) -> None:
    # Session A backgrounds a slow job; session B awaits it with a short
    # wait_for. B's timeout must expire WITHOUT cancelling A's job, which then
    # runs to completion.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> runtime.Job:
        job = await runtime.__ix_run(
            "await asyncio.sleep(0.3)\n'A finished'", budget=0.01, session="agent-a"
        )
        assert job.running()
        with pytest.raises(TimeoutError):
            await asyncio.wait_for(runtime.jobs[job.id], timeout=0.05)
        assert job.status == "running"  # the wait died; the job must not have
        await job.wait(10)
        return job

    job = asyncio.run(scenario())
    assert job.status == "done"
    assert "A finished" in (job.text or "")


def test_cancelling_a_job_errors_its_awaiters_instead_of_cancelling_them(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Two other sessions await one shared job; its owner cancels it. The
    # awaiting cells must record an ERROR naming the cancelled job -- not be
    # marked "cancelled" themselves.
    _wire(monkeypatch, {"asyncio": asyncio, "jobs": runtime.jobs})

    async def scenario() -> tuple[runtime.Job, runtime.Job, runtime.Job]:
        slow = await runtime.__ix_run(
            "await asyncio.sleep(30)", budget=0.01, session="agent-a"
        )
        w1 = await runtime.__ix_run(
            f"await jobs[{slow.id!r}]", budget=0.01, session="agent-b"
        )
        w2 = await runtime.__ix_run(
            f"await jobs[{slow.id!r}]", budget=0.01, session="agent-c"
        )
        assert slow.running()
        assert w1.running()
        assert w2.running()
        slow.cancel()
        await w1.wait(10)
        await w2.wait(10)
        return slow, w1, w2

    slow, w1, w2 = asyncio.run(scenario())
    assert slow.status == "cancelled"
    for waiter in (w1, w2):
        assert waiter.status == "error"
        assert slow.id in (waiter.error or "")


def test_awaiting_an_already_cancelled_job_raises_job_cancelled(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> None:
        slow = await runtime.__ix_run(
            "await asyncio.sleep(30)", budget=0.01, session="agent-a"
        )
        slow.cancel()
        await slow.wait(10)
        assert slow.status == "cancelled"
        with pytest.raises(runtime.JobCancelled, match=slow.id):
            await runtime.jobs[slow.id]

    asyncio.run(scenario())


def test_cancelling_an_awaiting_cell_leaves_the_job_running(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The inverse direction: cancelling the WAITER cell must stop only that
    # cell; the awaited job keeps running and finishes.
    _wire(monkeypatch, {"asyncio": asyncio, "jobs": runtime.jobs})

    async def scenario() -> runtime.Job:
        slow = await runtime.__ix_run(
            "await asyncio.sleep(0.3)\n'still here'", budget=0.01, session="agent-a"
        )
        waiter = await runtime.__ix_run(
            f"await jobs[{slow.id!r}]", budget=0.01, session="agent-b"
        )
        assert waiter.running()
        waiter.cancel()
        await waiter.wait(10)
        assert waiter.status == "cancelled"
        assert slow.status == "running"
        await slow.wait(10)
        return slow

    slow = asyncio.run(scenario())
    assert slow.status == "done"
