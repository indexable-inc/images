"""The one shared IPython kernel and the bridge that drives it.

There is exactly one kernel for the server's lifetime (the design choice: one
kernel, one namespace, async concurrency on its event loop). ``python_exec``
sends ``await __ix_exec(<code>, budget=...)``; the kernel-side runtime runs the
code as a task, waits the budget, and emits a structured summary plus the
result's rich output, which this module collects and hands back.

A single asyncio lock serializes the shell channel (a kernel processes one
``execute_request`` at a time): the *budget* keeps each request short by design,
so backgrounded work never holds the channel, and a later ``python_exec`` that
inspects ``jobs`` gets serviced promptly.
"""

from __future__ import annotations

import asyncio
import os
import signal
from pathlib import Path

from .config import Config, runtime_dir
from .outputs import job_summary, output_from_message

_READY_TIMEOUT = 60.0

# Env var carrying the path the kernel's faulthandler writes all-thread stacks to
# on SIGUSR1. The server sets it before launching the kernel and reads it back in
# ``dump_trace``; the kernel-side runtime registers the handler (``runtime``).
TRACE_ENV = "IX_MCP_KERNEL_TRACE"


def _wedged_summary(budget: float, grace: float, deadline: float) -> dict:
    """A per-call summary, shaped like ``runtime._job_summary``, returned when a
    cell blocks the kernel past ``deadline``. The server renders it like any
    other summary, so the caller gets a clear, actionable message rather than an
    opaque transport timeout."""
    message = (
        f"Cell blocked the kernel's event loop for over {deadline:.0f}s "
        f"(budget {budget:.0f}s + {grace:.0f}s grace) with a synchronous "
        "call, so the budget could not background it. The kernel was interrupted "
        "and is usable again. Wrap blocking calls (subprocess.run, time.sleep, "
        "requests, heavy CPU) in `await asyncio.to_thread(...)` or use an async "
        "API, and run anything slow as a background job. The interrupted run is "
        "recoverable in this kernel via history() / jobs['<id>']."
    )
    return {
        "id": None,
        "name": None,
        "status": "wedged",
        "running": False,
        "output": message,
        "output_chars": len(message),
        "result": None,
        "result_chars": 0,
        "error": message,
    }


class Kernel:
    def __init__(self, config: Config) -> None:
        self._config = config
        self._km = None
        self._kc = None
        self._lock = asyncio.Lock()
        self._trace_path: Path | None = None
        self._pid: int | None = None

    async def start(self) -> None:
        from jupyter_client.manager import AsyncKernelManager

        # Point the kernel's faulthandler at a private file before launch; the
        # kernel inherits this env and registers the SIGUSR1 dump handler.
        self._trace_path = runtime_dir() / "kernel-trace.txt"
        os.environ[TRACE_ENV] = str(self._trace_path)

        self._km = AsyncKernelManager(kernel_name="python3")
        await self._km.start_kernel(cwd=str(self._config.workdir))
        self._pid = self._kernel_pid()
        self._kc = self._km.client()
        self._kc.start_channels()
        await self._kc.wait_for_ready(timeout=_READY_TIMEOUT)

    def _kernel_pid(self) -> int | None:
        """The kernel process's pid, so a trace signal targets that process alone
        (not the kernel's process group, whose default SIGUSR1 would terminate
        user-launched subprocesses)."""
        provisioner = getattr(self._km, "provisioner", None)
        pid = getattr(provisioner, "pid", None)
        if pid is None:
            pid = getattr(getattr(self._km, "kernel", None), "pid", None)
        return pid

    async def dump_trace(self, timeout: float = 5.0) -> str:
        """All-thread Python stack of the kernel, captured via faulthandler on
        SIGUSR1. Works even when a synchronous call has wedged the event loop:
        the C-level handler runs in signal context, so it dumps while the main
        thread is still parked in the blocking call. Returns the newest dump."""
        if self._km is None or self._trace_path is None or self._pid is None:
            return "kernel is not running"
        path = self._trace_path
        before = path.stat().st_size if path.exists() else 0
        try:
            os.kill(self._pid, signal.SIGUSR1)
        except ProcessLookupError:
            return "kernel process is not alive"
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout
        while loop.time() < deadline:
            await asyncio.sleep(0.05)
            if path.exists() and path.stat().st_size > before:
                # A short settle so the whole multi-thread dump has flushed.
                await asyncio.sleep(0.05)
                return path.read_text()[before:].strip() or "(empty trace)"
        return (
            f"No trace was produced within {timeout:.0f}s. The kernel may not have "
            "the faulthandler registered (older build) or cannot service signals."
        )

    async def _execute(self, code: str, timeout: float) -> tuple[list[dict], dict | None]:
        async with self._lock:
            outputs: list[dict] = []
            summary: dict | None = None

            def on_iopub(msg: dict) -> None:
                nonlocal summary
                output = output_from_message(msg)
                if output is None:
                    return
                found = job_summary(output)
                if found is not None:
                    summary = found
                outputs.append(output)

            await self._kc.execute_interactive(
                code, timeout=timeout, allow_stdin=False, output_hook=on_iopub, store_history=True
            )
            return outputs, summary

    async def python_exec(self, code: str, budget: float, name: str | None = None) -> tuple[list[dict], dict | None]:
        """Run user ``code`` with a foreground budget; return (outputs, summary).

        ``code`` is passed as a repr-encoded string literal so any quoting is
        safe. A healthy cell completes within ``budget`` (the runtime backgrounds
        the job and returns the summary right after the budget elapses). If the
        kernel does not report idle within ``budget + wedge_grace`` the cell is
        blocking the kernel's single event loop with a synchronous call: interrupt
        the kernel so it is usable again and return an actionable summary instead
        of letting an opaque ``Timeout waiting for output`` escape to the caller.
        """
        name_arg = "None" if name is None else repr(name)
        wrapper = f"await __ix_exec({code!r}, budget={float(budget)!r}, name={name_arg})"
        grace = self._config.wedge_grace
        deadline = float(budget) + grace
        try:
            return await self._execute(wrapper, timeout=deadline)
        except TimeoutError:
            await self._interrupt()
            return [], _wedged_summary(budget, grace, deadline)

    async def _interrupt(self) -> None:
        """Break a synchronous call wedging the kernel's event loop. ipykernel's
        own ``interrupt_kernel`` cancels the asyncio task, which a synchronous call
        never yields to, so it cannot break a wedged async cell. Send SIGUSR2 to
        the kernel's runtime handler instead: it raises ``KeyboardInterrupt`` inline
        at the blocked frame, which ``_runner`` records as a failed job so the
        kernel returns to idle and the next call runs."""
        if self._pid is None:
            return
        try:
            os.kill(self._pid, signal.SIGUSR2)
        except ProcessLookupError:
            pass

    async def restart(self) -> None:
        if self._km is not None:
            await self._km.restart_kernel(now=True)
            await self._kc.wait_for_ready(timeout=_READY_TIMEOUT)

    async def shutdown(self) -> None:
        if self._kc is not None:
            self._kc.stop_channels()
        if self._km is not None:
            await self._km.shutdown_kernel(now=True)


_KERNEL: Kernel | None = None


def set_kernel(kernel: Kernel) -> None:
    global _KERNEL
    _KERNEL = kernel


def current_kernel() -> Kernel:
    if _KERNEL is None:
        raise RuntimeError("the kernel is not running; call a tool inside `ix-mcp serve`")
    return _KERNEL
