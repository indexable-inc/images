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

from .config import Config
from .outputs import job_summary, output_from_message

_READY_TIMEOUT = 60.0


class Kernel:
    def __init__(self, config: Config) -> None:
        self._config = config
        self._km = None
        self._kc = None
        self._lock = asyncio.Lock()

    async def start(self) -> None:
        from jupyter_client.manager import AsyncKernelManager

        self._km = AsyncKernelManager(kernel_name="python3")
        await self._km.start_kernel(cwd=str(self._config.workdir))
        self._kc = self._km.client()
        self._kc.start_channels()
        await self._kc.wait_for_ready(timeout=_READY_TIMEOUT)

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
        safe. The server timeout exceeds the budget so the wrapper cell (which
        returns right after the budget elapses) always completes in time.
        """
        name_arg = "None" if name is None else repr(name)
        wrapper = f"await __ix_exec({code!r}, budget={float(budget)!r}, name={name_arg})"
        return await self._execute(wrapper, timeout=float(budget) + 30.0)

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
