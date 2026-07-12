"""A client that abandons an in-flight ``python_exec`` cancels the run it
launched, instead of letting it finish in the background (issue #2387), and
cancels STRICTLY that run rather than a heuristic "newest running job" that
would kill an unrelated background job the same session started (issue #2406).

The MCP server dispatches a ``python_exec`` to the kernel, which runs the code
as a background ``Job`` once its foreground budget elapses. If the client then
cancels the request (``notifications/cancelled`` or a transport abort), the tool
coroutine is cancelled server-side -- but the kernel job used to keep running,
executing side effects the caller already abandoned (the permission-gate bypass
in #2387: a rejected/abandoned ``home-manager switch`` still built).

``__ix_cancel_running`` is what the server pokes on the raw shell channel when a
call is cancelled. It takes the launched run's ``job_id`` (read off the drained
exec reply and plumbed through by the server) and cancels STRICTLY that job, on
the same path as an explicit ``jobs['<id>'].cancel()``. It never falls back to
"the newest running job for the session": that heuristic deterministically kills
an unrelated background job whenever the abandoned call finished within budget
(#2406). These tests exercise it directly against the runtime (no kernel
process, no loopback bind -- so they pass under the darwin sandbox in nix
checks).
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
    # The client abandons the call; the server cancels the run it launched by id,
    # and the job must stop instead of finishing.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> runtime.Job:
        job = await runtime.__ix_run(
            "await asyncio.sleep(30)\n'side effect ran'",
            budget=0.01,
            session="agent-a",
        )
        assert job.running()
        cancelled = runtime.__ix_cancel_running(session="agent-a", job_id=job.id)
        assert cancelled == [job.id]
        await job.wait(10)
        return job

    job = asyncio.run(scenario())
    assert job.status == "cancelled"
    assert "side effect ran" not in (job.text or "")


def test_cancel_running_targets_only_the_launched_run(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A session may have an earlier legitimate background job still running. Only
    # the run named by ``job_id`` (the abandoned call) is cancelled; the earlier
    # one keeps running -- even though it started FIRST, so a "newest wins"
    # heuristic would have gotten this right too. The next test covers the case
    # the heuristic gets wrong.
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
        cancelled = runtime.__ix_cancel_running(session="agent-a", job_id=newest.id)
        assert cancelled == [newest.id]
        await earlier.wait(10)
        return earlier, newest

    earlier, newest = asyncio.run(scenario())
    assert newest.status == "cancelled"
    assert earlier.status == "done"
    assert "earlier done" in (earlier.text or "")


def test_cancel_running_does_not_kill_an_unrelated_job_when_the_abandoned_run_finished(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The wrong-job kill from #2406. The abandoned ``python_exec`` was a quick
    # call that FINISHED within its foreground budget (so its summary is drained
    # and its id is known), while an OLDER background job the same session
    # started earlier is still running. The old heuristic ("cancel the newest
    # still-running job for the session") had no running quick call left to find,
    # so it fell through and cancelled the older long-running job -- a
    # deterministic wrong-job kill. Cancelling strictly by the finished run's id
    # is a no-op, and the older job keeps running.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> tuple[runtime.Job, runtime.Job]:
        older = await runtime.__ix_run(
            "await asyncio.sleep(0.4)\n'older done'", budget=0.01, session="agent-a"
        )
        await asyncio.sleep(0.02)  # strictly later start for the quick call
        quick = await runtime.__ix_run("1 + 1", budget=5.0, session="agent-a")
        assert quick.status == "done"  # finished within budget
        assert older.running()
        # By-id cancel of the finished quick run: nothing to cancel, and the
        # older running job is NOT touched (the newest-heuristic misfire).
        assert runtime.__ix_cancel_running(session="agent-a", job_id=quick.id) == []
        assert older.running()
        # And the by-id cancel of an actually-abandoned running job still works:
        running = await runtime.__ix_run(
            "await asyncio.sleep(30)", budget=0.01, session="agent-a"
        )
        assert running.running()
        assert runtime.__ix_cancel_running(session="agent-a", job_id=running.id) == [
            running.id
        ]
        await running.wait(10)
        await older.wait(10)
        return older, running

    older, running = asyncio.run(scenario())
    assert older.status == "done"
    assert "older done" in (older.text or "")
    assert running.status == "cancelled"


def test_cancel_running_without_a_job_id_cancels_nothing(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Some cancel paths have no summary to read the id from -- the cancel landed
    # before any iopub carried the launched job's summary. With no id to target,
    # the helper must cancel NOTHING rather than guess and risk the wrong-job
    # kill; the running job is left alone.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> runtime.Job:
        job = await runtime.__ix_run(
            "await asyncio.sleep(30)", budget=0.01, session="agent-a"
        )
        assert job.running()
        assert runtime.__ix_cancel_running(session="agent-a", job_id=None) == []
        assert job.running()
        job.cancel()
        await job.wait(10)
        return job

    job = asyncio.run(scenario())
    assert job.status == "cancelled"


def test_cancel_running_leaves_other_sessions_untouched(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Cancelling one session's abandoned run must never touch another session's
    # job -- the same isolation issue #2104 protects. Even a job_id that names
    # another session's run is refused by the session guard.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> tuple[runtime.Job, runtime.Job]:
        mine = await runtime.__ix_run(
            "await asyncio.sleep(30)", budget=0.01, session="agent-a"
        )
        other = await runtime.__ix_run(
            "await asyncio.sleep(0.2)\n'other done'", budget=0.01, session="agent-b"
        )
        # A job_id from another session is refused by the session guard.
        assert runtime.__ix_cancel_running(session="agent-a", job_id=other.id) == []
        assert other.running()
        cancelled = runtime.__ix_cancel_running(session="agent-a", job_id=mine.id)
        assert cancelled == [mine.id]
        await mine.wait(10)
        await other.wait(10)
        return mine, other

    mine, other = asyncio.run(scenario())
    assert mine.status == "cancelled"
    assert other.status == "done"
    assert "other done" in (other.text or "")


def test_cancel_running_spares_a_job_the_call_deliberately_spawned(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # A python_exec call may detach work with ``jobs.spawn`` -- the user asked
    # for that to outlive the call (index#2387/#2401). The spawned child inherits
    # the parent's session and starts LATER than the parent run, so a naive
    # "cancel the newest running job for this session" would kill the child
    # instead of the abandoned foreground run. Cancelling by the foreground
    # run's id spares the child by construction; this test pins that property.
    _wire(monkeypatch, {"asyncio": asyncio, "jobs": runtime.jobs})

    async def scenario() -> tuple[runtime.Job, runtime.Job]:
        foreground = await runtime.__ix_run(
            "child = jobs.spawn(asyncio.sleep(30))\n"
            "await asyncio.sleep(30)\n"
            "'foreground side effect ran'",
            budget=0.05,
            session="agent-a",
        )
        assert foreground.running()

        # The cell spawns the child on its FIRST line, but the foreground run may
        # background (budget elapsed) before the cell task has run that far --
        # more likely under a loaded, single-core CI runner. Wait for the child
        # to actually register rather than assuming it is there already.
        def _spawned() -> list[runtime.Job]:
            return [
                j
                for j in runtime.jobs.values()
                if j.session == "agent-a" and j.kind == "spawn"
            ]

        for _ in range(1000):
            if _spawned():
                break
            await asyncio.sleep(0.005)
        children = _spawned()
        assert children, "the cell's jobs.spawn child never registered"
        spawned = max(children, key=lambda j: j.started)
        assert spawned.started > foreground.started  # newest running is the child
        cancelled = runtime.__ix_cancel_running(
            session="agent-a", job_id=foreground.id
        )
        assert cancelled == [foreground.id]  # NOT the spawned child
        await foreground.wait(10)
        # Assert the child survives WHILE the loop is still live: `asyncio.run`
        # cancels every outstanding task at teardown, so a check after it
        # returned would see the child cancelled by the shutdown, not by us.
        assert foreground.status == "cancelled"
        assert spawned.running()  # the deliberately-detached job was spared
        spawned.cancel()  # cleanup before the loop closes
        return foreground, spawned

    foreground, _spawned = asyncio.run(scenario())
    assert foreground.status == "cancelled"
    assert "foreground side effect ran" not in (foreground.text or "")


def test_cancel_running_is_a_noop_when_the_run_already_finished(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The common race: a fast call finishes before the cancellation lands. Even
    # with its id in hand there is nothing to cancel, and the helper must return
    # an empty list rather than touch an already-done job.
    _wire(monkeypatch, {"asyncio": asyncio})

    async def scenario() -> runtime.Job:
        job = await runtime.__ix_run("1 + 1", budget=5.0, session="agent-a")
        assert job.status == "done"
        assert runtime.__ix_cancel_running(session="agent-a", job_id=job.id) == []
        return job

    job = asyncio.run(scenario())
    assert job.status == "done"
