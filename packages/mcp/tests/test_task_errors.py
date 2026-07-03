"""A fire-and-forget task that dies must be reported at completion, not GC.

Regression for 2026-07-02: a watcher task held by a namespace variable raised
an AttributeError and was never reported (CPython only warns at GC, and a live
reference prevents GC forever), starving external monitors for 90 minutes.
"""

from __future__ import annotations

import asyncio

import pytest

from ix_notebook_mcp import runtime


def test_result_output_aliases_rendered_text() -> None:
    result = runtime.Result.text("hello")
    assert result.output == "hello"
    assert result.output == result.llm_result


def test_unretrieved_task_failure_is_reported_even_with_live_reference() -> None:
    async def scenario() -> list[str]:
        runtime._install_task_failure_watch(asyncio.get_running_loop())
        runtime.task_errors.clear()

        async def boom() -> None:
            raise AttributeError("'Result' object has no attribute 'output'")

        # The failure mode under test: a strong reference keeps the task alive,
        # so the GC-time warning would never fire.
        held = asyncio.create_task(boom(), name="watcher")
        await asyncio.sleep(runtime._TASK_FAILURE_GRACE_S + 1.0)
        assert held is not None  # keep the reference genuinely live past the grace period
        return list(runtime.task_errors)

    reports = asyncio.run(scenario())
    assert len(reports) == 1
    assert "'watcher'" in reports[0]
    assert "AttributeError" in reports[0]


def test_retrieved_task_failure_is_not_reported() -> None:
    async def scenario() -> list[str]:
        runtime._install_task_failure_watch(asyncio.get_running_loop())
        runtime.task_errors.clear()

        async def boom() -> None:
            raise ValueError("handled")

        task = asyncio.create_task(boom())
        with pytest.raises(ValueError, match="handled"):
            await task  # prompt retrieval: the parent owns this failure
        await asyncio.sleep(runtime._TASK_FAILURE_GRACE_S + 1.0)
        return list(runtime.task_errors)

    assert asyncio.run(scenario()) == []


def test_cancelled_task_is_not_reported() -> None:
    async def scenario() -> list[str]:
        runtime._install_task_failure_watch(asyncio.get_running_loop())
        runtime.task_errors.clear()

        task = asyncio.create_task(asyncio.sleep(60))
        task.cancel()
        await asyncio.sleep(runtime._TASK_FAILURE_GRACE_S + 1.0)
        return list(runtime.task_errors)

    assert asyncio.run(scenario()) == []


def test_install_is_idempotent() -> None:
    """Re-running install() must not stack watcher factories (a stack doubles
    the per-task callbacks and timers, and a report per stacked layer)."""

    async def scenario() -> tuple[object, list[str]]:
        loop = asyncio.get_running_loop()
        runtime._install_task_failure_watch(loop)
        first = loop.get_task_factory()
        runtime._install_task_failure_watch(loop)
        runtime.task_errors.clear()

        async def boom() -> None:
            raise RuntimeError("once")

        held = asyncio.create_task(boom())
        await asyncio.sleep(runtime._TASK_FAILURE_GRACE_S + 1.0)
        assert held is not None
        return loop.get_task_factory(), list(runtime.task_errors)

    factory, reports = asyncio.run(scenario())
    assert len(reports) == 1
    assert factory is not None
    assert getattr(factory, "_ix_task_watch", False)


def test_second_install_leaves_factory_untouched() -> None:
    """A double-wrap would produce a NEW factory object that also carries the
    sentinel and still deduplicates reports, so only object identity catches
    an accidental re-wrap."""

    async def scenario() -> tuple[object, object]:
        loop = asyncio.get_running_loop()
        runtime._install_task_failure_watch(loop)
        first = loop.get_task_factory()
        runtime._install_task_failure_watch(loop)
        return first, loop.get_task_factory()

    first, second = asyncio.run(scenario())
    assert second is first
