"""A client that abandons an in-flight ``python_exec`` cancels the run it
launched, instead of letting it finish in the background (issue #2387).

The MCP server dispatches a ``python_exec`` to the kernel, which runs the code
as a background ``Job`` once its foreground budget elapses. If the client then
cancels the request (``notifications/cancelled`` or a transport abort), the tool
coroutine is cancelled server-side -- but the kernel job used to keep running,
executing side effects the caller already abandoned (the permission-gate bypass
in the issue: a rejected/abandoned ``home-manager switch`` still built).

``__ix_cancel_running`` is what the server pokes on the raw shell channel when a
call is cancelled: it cancels the single most-recently-started running job for
that session, on the same path as an explicit ``jobs['<id>'].cancel()``. These
tests exercise it directly against the runtime (no kernel process, no loopback
bind -- so they pass under the darwin sandbox in nix checks).
"""

from __future__ import annotations

import asyncio

import pytest

from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


def test_cancel_running_stops_the_abandoned_background_job(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A session backgrounds a long side-effectful run (stand-in: a long sleep).
    # The client abandons the call; the server cancels the run it launched, and
    # the job must stop instead of finishing.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> runtime.Job:
        job = await runtime.__ix_run(
            "await asyncio.sleep(30)\n'side effect ran'",
            budget=0.01,
            session="agent-a",
        )
        assert job.running()
        cancelled = runtime.__ix_cancel_running(session="agent-a")
        assert cancelled == [job.id]
        await job.wait(10)
        return job

    job = asyncio.run(scenario())
    assert job.status == "cancelled"
    assert "side effect ran" not in (job.text or "")


def test_cancel_running_targets_only_the_newest_run_for_the_session(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A session may have an earlier legitimate background job still running. Only
    # the most recently launched run (the abandoned call) is cancelled; the
    # earlier one keeps running.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> tuple[runtime.Job, runtime.Job]:
        earlier = await runtime.__ix_run(
            "await asyncio.sleep(0.3)\n'earlier done'", budget=0.01, session="agent-a"
        )
        await asyncio.sleep(0.02)  # ensure a strictly later start time
        newest = await runtime.__ix_run(
            "await asyncio.sleep(30)", budget=0.01, session="agent-a"
        )
        assert earlier.running()
        assert newest.running()
        cancelled = runtime.__ix_cancel_running(session="agent-a")
        assert cancelled == [newest.id]
        await earlier.wait(10)
        return earlier, newest

    earlier, newest = asyncio.run(scenario())
    assert newest.status == "cancelled"
    assert earlier.status == "done"
    assert "earlier done" in (earlier.text or "")


def test_cancel_running_leaves_other_sessions_untouched(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Cancelling one session's abandoned run must never touch another session's
    # job -- the same isolation issue #2104 protects.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> tuple[runtime.Job, runtime.Job]:
        mine = await runtime.__ix_run(
            "await asyncio.sleep(30)", budget=0.01, session="agent-a"
        )
        other = await runtime.__ix_run(
            "await asyncio.sleep(0.2)\n'other done'", budget=0.01, session="agent-b"
        )
        cancelled = runtime.__ix_cancel_running(session="agent-a")
        assert cancelled == [mine.id]
        await mine.wait(10)
        await other.wait(10)
        return mine, other

    mine, other = asyncio.run(scenario())
    assert mine.status == "cancelled"
    assert other.status == "done"
    assert "other done" in (other.text or "")


def test_cancel_running_is_a_noop_when_the_run_already_finished(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The common race: a fast call finishes before the cancellation lands. There
    # is nothing to cancel, and the helper must return an empty list rather than
    # touch an already-done job.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> runtime.Job:
        job = await runtime.__ix_run("1 + 1", budget=5.0, session="agent-a")
        assert job.status == "done"
        assert runtime.__ix_cancel_running(session="agent-a") == []
        return job

    job = asyncio.run(scenario())
    assert job.status == "done"
