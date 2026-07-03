"""Awaiting a failed job re-raises, and the Job/Result accessor surfaces line up
(issue #1754, bugs 1-2).

Bug 1: a job that failed used to yield ``None`` from ``await jobs[id]`` -- so
``(await jobs[id]).text`` blew up with an opaque ``AttributeError`` -- which
inverts the documented "raises rather than return a misleading None" contract.
Awaiting a failed job now re-raises the original exception (type, message, and the
cell's own traceback).

Bug 2: a live/errored ``Job`` exposes ``.output``; a finished ``Result`` exposes
``.text``. Each now also answers its sibling (``Result.output``, ``Job.text``), so
an agent paging a returned value does not have to guess which surface owns which
name.
"""

from __future__ import annotations

import asyncio

import pytest

from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


def _run(code: str) -> runtime.Job:
    return asyncio.run(runtime.__ix_run(code, budget=5.0))


def test_awaiting_a_failed_job_reraises_the_original_exception(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})
    job = _run("raise ValueError('boom')")
    assert job.status == "error"

    async def await_it() -> object:
        return await runtime.jobs[job.id]

    with pytest.raises(ValueError, match="boom"):
        asyncio.run(await_it())


def test_the_reproduced_production_failure_no_longer_gives_none(monkeypatch: pytest.MonkeyPatch) -> None:
    # The exact prod shape: a cell fails, then `r = await jobs[id]; r.text`. The
    # await must raise, never hand back a None whose `.text` dies with AttributeError.
    _wire(monkeypatch, {})
    job = _run("raise RuntimeError('rg exploded')")

    async def await_and_read_text() -> str:
        r = await runtime.jobs[job.id]
        return r.text  # this used to be `None.text` -> AttributeError

    with pytest.raises(RuntimeError, match="rg exploded"):
        asyncio.run(await_and_read_text())


def test_awaiting_a_successful_job_yields_the_result(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {"Result": runtime.Result})
    job = _run("Result.text('done well')")

    async def await_it() -> runtime.Result:
        return await runtime.jobs[job.id]

    result = asyncio.run(await_it())
    assert isinstance(result, runtime.Result)
    assert result.text == "done well"


def test_result_output_aliases_text_and_llm_result(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {"Result": runtime.Result})
    job = _run("Result.text('hello')")
    result = job.result
    assert result.output == result.text == result.llm_result == "hello"


def test_job_text_is_the_result_text(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {"Result": runtime.Result})
    job = _run("Result.text('rendered')")
    # `.text` is the sibling of `.output` (stdout): the finished run's result text.
    assert job.text == "rendered"
    assert job.output == ""  # the cell printed nothing to stdout


def test_a_watchdog_interrupt_reraises_with_the_actionable_message(monkeypatch: pytest.MonkeyPatch) -> None:
    # Simulate the wedge watchdog: a job flagged interrupted_by_watchdog whose
    # KeyboardInterrupt is caught should keep the actionable message AND re-raise.
    _wire(monkeypatch, {})

    async def run_wedged() -> runtime.Job:
        job = runtime.Job("raise KeyboardInterrupt()", budget=5.0)
        job.interrupted_by_watchdog = True
        runtime.jobs[job.id] = job
        job.task = asyncio.ensure_future(runtime._runner(job, {}))
        await job.task
        return job

    job = asyncio.run(run_wedged())
    assert job.status == "error"
    assert job._exc is not None
    assert "blocking the" in (job.error or "")
