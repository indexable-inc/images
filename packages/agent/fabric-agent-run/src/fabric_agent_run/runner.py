"""One call-owned state machine from submission through a terminal fact."""

from __future__ import annotations

import asyncio
import os
import platform
from collections.abc import Awaitable, Callable, Mapping
from dataclasses import dataclass
from enum import StrEnum
from pathlib import Path

from .backends import Backend, BackendSpec, spawn_backend
from .journal import Fact, Journal, WeaveJournal, mint_task, wait_for_interrupt

_POLL_SECONDS = 0.5


class RunState(StrEnum):
    DONE = "done"
    FAILED = "failed"
    INTERRUPTED = "interrupted"


@dataclass(frozen=True)
class AgentSpec:
    name: str
    harness: str
    model: str
    prompt: bytes
    cwd: Path
    timeout_seconds: float
    effort: str | None = None
    requested_by: str = "fabric-agent-run"
    max_result_bytes: int = 16 * 1024 * 1024


@dataclass(frozen=True)
class Outcome:
    task: str
    state: RunState
    result: str | None = None
    error: str | None = None


BackendFactory = Callable[[BackendSpec, bytes, Mapping[str, str]], Awaitable[Backend]]


async def _default_backend_factory(
    spec: BackendSpec,
    prompt: bytes,
    environ: Mapping[str, str],
) -> Backend:
    return await spawn_backend(spec, prompt, environ=environ)


async def _write_terminal(
    journal: Journal,
    task: str,
    state: RunState,
    *,
    result: str | None = None,
    error: str | None = None,
) -> Outcome:
    facts: list[Fact] = []
    if result is not None:
        result_hash = await journal.put_blob(result.encode())
        facts.append((task, "result", result_hash))
    if error is not None:
        facts.append((task, "error", error))
    facts.append((task, "state", state.value))
    await journal.assert_facts(facts)
    return Outcome(task=task, state=state, result=result, error=error)


async def run_agent(
    spec: AgentSpec,
    *,
    journal: Journal | None = None,
    backend_factory: BackendFactory = _default_backend_factory,
    stop: asyncio.Event | None = None,
    environ: Mapping[str, str] | None = None,
    poll_seconds: float = _POLL_SECONDS,
) -> Outcome:
    """Own one agent call until ``done``, ``failed``, or ``interrupted``."""

    owned_record: WeaveJournal | None = None
    if journal is None:
        owned_record = WeaveJournal()
        record: Journal = owned_record
    else:
        record = journal
    environment = environ or os.environ
    prompt_hash = await record.put_blob(spec.prompt)
    task = mint_task()
    await record.assert_facts(
        [
            (task, "type", "task"),
            (task, "fn", f"agent.{spec.harness}"),
            (task, "name", spec.name),
            (task, "harness", spec.harness),
            (task, "model", spec.model),
            *([(task, "effort", spec.effort)] if spec.effort is not None else []),
            (task, "node", platform.node()),
            (task, "requested_by", spec.requested_by),
            (task, "prompt", prompt_hash),
            (task, "state", "submitted"),
        ]
    )

    backend_spec = BackendSpec(
        harness=spec.harness,
        model=spec.model,
        effort=spec.effort,
        cwd=spec.cwd,
        max_result_bytes=spec.max_result_bytes,
    )
    backend: Backend | None = None
    result_task: asyncio.Task[str] | None = None
    interrupt_task: asyncio.Task[None] | None = None
    signal_task: asyncio.Task[bool] | None = None
    timeout_task: asyncio.Task[None] | None = None
    try:
        loop = asyncio.get_running_loop()
        deadline = loop.time() + spec.timeout_seconds
        async with asyncio.timeout(spec.timeout_seconds):
            backend = await backend_factory(backend_spec, spec.prompt, environment)
        await record.assert_facts([(task, "state", "running")])
        result_task = asyncio.create_task(backend.result(), name=f"fabric-agent:{task}")
        interrupt_task = asyncio.create_task(
            wait_for_interrupt(record, task, poll_seconds=poll_seconds),
            name=f"fabric-agent:interrupt:{task}",
        )
        if stop is not None:
            signal_task = asyncio.create_task(stop.wait(), name=f"fabric-agent:signal:{task}")
        timeout_task = asyncio.create_task(
            asyncio.sleep(max(0.0, deadline - loop.time())),
            name=f"fabric-agent:timeout:{task}",
        )
        waiters: set[asyncio.Task[object]] = {result_task, interrupt_task, timeout_task}
        if signal_task is not None:
            waiters.add(signal_task)
        done, _pending = await asyncio.wait(waiters, return_when=asyncio.FIRST_COMPLETED)

        if result_task in done:
            try:
                result = result_task.result()
            except BaseException as exc:
                return await _write_terminal(
                    record,
                    task,
                    RunState.FAILED,
                    error=f"{type(exc).__name__}: {exc}",
                )
            return await _write_terminal(record, task, RunState.DONE, result=result)

        await backend.interrupt()
        await asyncio.gather(result_task, return_exceptions=True)
        if interrupt_task in done:
            watch_error = interrupt_task.exception()
            if watch_error is not None:
                return await _write_terminal(
                    record,
                    task,
                    RunState.FAILED,
                    error=f"interrupt watch failed: {type(watch_error).__name__}: {watch_error}",
                )
        if timeout_task in done:
            return await _write_terminal(
                record,
                task,
                RunState.FAILED,
                error=f"TimeoutError: agent did not finish within {spec.timeout_seconds:g}s",
            )
        return await _write_terminal(record, task, RunState.INTERRUPTED)
    except asyncio.CancelledError:
        if backend is not None:
            await asyncio.shield(backend.interrupt())
        await asyncio.shield(_write_terminal(record, task, RunState.INTERRUPTED))
        raise
    except TimeoutError:
        return await _write_terminal(
            record,
            task,
            RunState.FAILED,
            error=f"TimeoutError: agent did not start within {spec.timeout_seconds:g}s",
        )
    except BaseException as exc:
        return await _write_terminal(
            record,
            task,
            RunState.FAILED,
            error=f"{type(exc).__name__}: {exc}",
        )
    finally:
        for pending in (interrupt_task, signal_task, timeout_task):
            if pending is not None:
                pending.cancel()
        if backend is not None:
            await backend.close()
        if owned_record is not None:
            await owned_record.close()
