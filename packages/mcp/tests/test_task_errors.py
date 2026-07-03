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
        await asyncio.sleep(runtime._TASK_FAILURE_GRACE_S + 0.5)
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
        await asyncio.sleep(runtime._TASK_FAILURE_GRACE_S + 0.5)
        return list(runtime.task_errors)

    assert asyncio.run(scenario()) == []


def test_cancelled_task_is_not_reported() -> None:
    async def scenario() -> list[str]:
        runtime._install_task_failure_watch(asyncio.get_running_loop())
        runtime.task_errors.clear()

        task = asyncio.create_task(asyncio.sleep(60))
        task.cancel()
        await asyncio.sleep(runtime._TASK_FAILURE_GRACE_S + 0.5)
        return list(runtime.task_errors)

    assert asyncio.run(scenario()) == []
