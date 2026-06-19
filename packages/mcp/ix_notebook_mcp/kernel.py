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
import contextlib
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


def _wedged_summary(budget: float, grace: float, deadline: float, *, interrupted: bool) -> dict:
    """A per-call summary, shaped like ``runtime._job_summary``, returned when a
    cell blocks the kernel past ``deadline``. The server renders it like any
    other summary, so the caller gets a clear, actionable message rather than an
    opaque transport timeout. ``interrupted`` reports whether the rescue signal
    was actually sent, so the message does not claim a recovery that did not
    happen when the kernel pid is unknown."""
    recovery = (
        "The kernel was interrupted and is usable again."
        if interrupted
        else "The kernel could NOT be interrupted (its pid is unknown); it is still "
        "blocked and likely needs a restart."
    )
    message = (
        f"Cell blocked the kernel's event loop for over {deadline:.0f}s "
        f"(budget {budget:.0f}s + {grace:.0f}s grace) with a synchronous "
        f"call, so the budget could not background it. {recovery} "
        "Wrap blocking calls (subprocess.run, time.sleep, "
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
        # The wall-clock seconds this call blocked before the server gave up, so a
        # wedged reply still carries elapsed_s (and reports the slowest case the
        # field exists to surface) rather than a misleading null.
        "elapsed_s": round(deadline, 2),
    }


class Kernel:
    def __init__(self, config: Config) -> None:
        self._config = config
        self._km = None
        self._kc = None
        self._lock = asyncio.Lock()
        self._trace_lock = asyncio.Lock()
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
        # Serialize dumps: two concurrent traces share the same `before` offset and
        # would each read both appended dumps. The lock keeps each dump clean.
        async with self._trace_lock:
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

    async def _execute(
        self, code: str, timeout: float, on_locked: object = None
    ) -> tuple[list[dict], dict | None]:
        async with self._lock:
            # `on_locked` fires once the shell channel is held: a caller that
            # must run BEFORE any later request (session restore) signals here,
            # and everything submitted afterwards queues behind this lock.
            if on_locked is not None:
                on_locked()
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

            # Run the request as a task and shield it from client-side
            # cancellation. A CancelledError thrown straight into
            # execute_interactive (the client cancels the python_exec call)
            # abandons a half-read multipart reply on the shared shell socket,
            # desyncing it so EVERY later python_exec hangs -- the "I cancelled and
            # now nothing runs" wedge. The cell self-backgrounds at its budget, so
            # the reply always arrives within ``timeout``; on cancel we still drain
            # it (lock held) before re-raising, leaving the channel clean.
            task = asyncio.ensure_future(
                self._kc.execute_interactive(
                    code, timeout=timeout, allow_stdin=False, output_hook=on_iopub, store_history=True
                )
            )
            try:
                await asyncio.shield(task)
            except asyncio.CancelledError:
                try:
                    await task
                except TimeoutError:
                    # The cell is synchronously wedging the loop, so the reply
                    # never arrives within the deadline. The drain alone would
                    # leave the kernel stuck behind the cancelled-but-still-running
                    # cell, so fire the same SIGUSR2 watchdog the outer timeout
                    # path uses to break the blocked frame and free the channel.
                    await self._interrupt()
                except BaseException:  # noqa: S110 -- any drain error is acceptable; we just need the socket read to finish before releasing the lock
                    # Any other drain error: we only need the socket read to
                    # finish before releasing the lock.
                    pass
                raise
            return outputs, summary

    async def python_exec(
        self, code: str, budget: float, name: str | None = None, session: str | None = None
    ) -> tuple[list[dict], dict | None]:
        """Run user ``code`` with a foreground budget; return (outputs, summary).

        ``code`` is passed as a repr-encoded string literal so any quoting is
        safe. ``session`` is the caller's MCP session id; the kernel runtime runs
        the code in that session's own namespace (None: the shared one), so
        parallel clients of one kernel do not clobber each other's variables. A
        healthy cell completes within ``budget`` (the runtime backgrounds
        the job and returns the summary right after the budget elapses). If the
        kernel does not report idle within ``budget + wedge_grace`` the cell is
        blocking the kernel's single event loop with a synchronous call: interrupt
        the kernel so it is usable again and return an actionable summary instead
        of letting an opaque ``Timeout waiting for output`` escape to the caller.
        """
        name_arg = "None" if name is None else repr(name)
        session_arg = "None" if session is None else repr(session)
        wrapper = (
            f"await __ix_exec({code!r}, budget={float(budget)!r}, "
            f"name={name_arg}, session={session_arg})"
        )
        grace = self._config.wedge_grace
        deadline = float(budget) + grace
        try:
            return await self._execute(wrapper, timeout=deadline)
        except TimeoutError:
            interrupted = await self._interrupt()
            return [], _wedged_summary(budget, grace, deadline, interrupted=interrupted)

    async def set_client(self, client: str) -> None:
        """Tell the kernel which MCP client connected, so the session label can
        default to it. Runs as a raw shell request (not ``__ix_exec``), so it
        leaves no job/card behind — it only pokes ``session._set_client``. The
        server calls this once, when the client identifies itself."""
        with contextlib.suppress(Exception):  # session label is a convenience; must not break the tool call
            await self._execute(f"session._set_client({client!r})", timeout=10.0)

    async def _interrupt(self) -> bool:
        """Break a synchronous call wedging the kernel's event loop. ipykernel's
        own ``interrupt_kernel`` cancels the asyncio task, which a synchronous call
        never yields to, so it cannot break a wedged async cell. Send SIGUSR2 to
        the kernel's runtime handler instead: it raises ``KeyboardInterrupt`` inline
        at the blocked frame, which ``_runner`` records as a failed job so the
        kernel returns to idle and the next call runs. Returns whether the signal
        was actually delivered, so the caller's summary does not claim a recovery
        that did not happen."""
        if self._pid is None:
            return False
        try:
            os.kill(self._pid, signal.SIGUSR2)
        except ProcessLookupError:
            return False
        return True

    async def restore_session(self, on_locked: object = None, timeout: float = 1800.0) -> str:
        """Reopen a session in the kernel: load the latest checkpoint and replay
        the gap (``__ix_restore`` in the runtime). Returns the printed summary.
        ``on_locked`` fires once the request holds the shell channel, so the
        caller can start serving tools immediately -- they queue behind this."""
        outputs, _ = await self._execute("await __ix_restore()", timeout=timeout, on_locked=on_locked)
        texts = [o.get("text", "") for o in outputs if isinstance(o, dict)]
        return "".join(t for t in texts if isinstance(t, str)).strip()

    async def snapshot_session(self) -> None:
        """Best-effort final checkpoint at shutdown, so the last cells' state is
        in the file even if the debounced checkpoint had not fired yet."""
        with contextlib.suppress(Exception):  # shutdown must proceed; periodic checkpoint plus replay guarantee a correct reopen
            await self._execute("await __ix_snapshot()", timeout=60.0)

    async def restart(self) -> None:
        if self._km is not None:
            await self._km.restart_kernel(now=True)
            # The restart launches a new process, so refresh the pid the trace and
            # interrupt signals target; a stale pid would signal a dead or reused
            # process. The new kernel re-runs install() and re-opens the trace file.
            self._pid = self._kernel_pid()
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
