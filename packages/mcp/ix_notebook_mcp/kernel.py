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

A death watch polls the kernel child for the server's lifetime: a kernel killed
externally (SIGTERM/SIGKILL, OOM, crash) is reported as ``kernel died (pid N,
signal S); respawning`` -- never as a generic wedge or transport timeout -- and
is respawned immediately, not lazily on the next execute (index#2339).
"""

from __future__ import annotations

import ast
import asyncio
import contextlib
import os
import signal
import sys
from pathlib import Path

from .config import Config, runtime_dir
from .outputs import job_summary, output_from_message

_READY_TIMEOUT = 60.0

# Env var carrying the path the kernel's faulthandler writes all-thread stacks to
# on SIGUSR1. The server sets it before launching the kernel and reads it back in
# ``dump_trace``; the kernel-side runtime registers the handler (``runtime``).
TRACE_ENV = "IX_MCP_KERNEL_TRACE"

# How often the death watch polls the kernel child (a waitpid(WNOHANG) via the
# provisioner: cheap). Small enough that an external kill surfaces as the precise
# death error within the same breath, not after a 35s transport timeout.
_WATCH_INTERVAL = 0.5


class KernelDiedError(RuntimeError):
    """The kernel process exited (killed externally, OOM, crashed) and the
    server is respawning it. Raised by the tool entry points so the caller gets
    the precise cause -- ``kernel died (pid N, signal S); respawning`` -- instead
    of a generic wedge or timeout against a process that no longer exists."""


def _describe_exit(returncode: int | None) -> str:
    """Render a Popen returncode: negative is death by that signal, by name."""
    if returncode is None:
        return "unknown exit"
    if returncode < 0:
        try:
            return f"signal {signal.Signals(-returncode).name}"
        except ValueError:
            return f"signal {-returncode}"
    return f"exit code {returncode}"


def trace_path_for(server_pid: int) -> Path:
    """The faulthandler dump target for the serve owning ``server_pid``. One
    file per serve, not one machine-wide name: concurrent kernels sharing a
    path truncate and interleave each other's dumps (index#2355)."""
    return runtime_dir() / f"kernel-trace-{server_pid}.txt"


def _sweep_stale_traces() -> None:
    """Drop trace files orphaned by serves that are gone: a SIGKILLed serve
    never reaches shutdown(), so its file would linger in runtime_dir()
    forever. The legacy fixed-name ``kernel-trace.txt`` (no pid suffix) is
    left alone for still-running older builds."""
    for path in runtime_dir().glob("kernel-trace-*.txt"):
        try:
            pid = int(path.stem.rsplit("-", 1)[-1])
        except ValueError:
            continue
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            path.unlink(missing_ok=True)
        except PermissionError:
            continue  # a live process we cannot signal: not ours to sweep


# What the wedge rescue actually achieved, verified rather than assumed
# (index#2375): "recovered" means the kernel answered a probe after the
# interrupt; "restarted" means it did not (a block inside native code never
# reaches the Python-level rescue handler) and the kernel child was killed and
# respawned; "restart_pending" means another call already owns that respawn.
_RECOVERY = {
    "recovered": "The kernel was interrupted, answered a follow-up probe, and is usable again.",
    "restarted": (
        "The interrupt did not unblock it (a block inside native code never "
        "reaches the Python-level rescue handler), so the kernel child was "
        "killed and respawned: the session namespace was restored from its "
        "checkpoint and this cell's work was lost."
    ),
    "restart_pending": (
        "The interrupt did not unblock it and a kernel restart is already in "
        "progress; retry shortly."
    ),
}


def _wedged_summary(budget: float, grace: float, deadline: float, *, outcome: str) -> dict:
    """A per-call summary, shaped like ``runtime._job_summary``, returned when a
    cell blocks the kernel past ``deadline``. The server renders it like any
    other summary, so the caller gets a clear, actionable message rather than an
    opaque transport timeout. ``outcome`` (a ``_RECOVERY`` key) reports what the
    rescue verifiably achieved, so the message never claims a recovery that did
    not happen (index#2375: SIGUSR2 delivery alone proves nothing when the block
    is in native code)."""
    message = (
        f"Cell blocked the kernel's event loop for over {deadline:.0f}s "
        f"(budget {budget:.0f}s + {grace:.0f}s grace) with a synchronous "
        f"call, so the budget could not background it. {_RECOVERY[outcome]} "
        "Wrap blocking calls (subprocess.run, time.sleep, "
        "requests, heavy CPU) in `await asyncio.to_thread(...)` or use an async "
        "API, and run anything slow as a background job."
    )
    if outcome == "recovered":
        # Only a surviving kernel still holds the interrupted run's job row.
        message += " The interrupted run is recoverable in this kernel via history() / jobs['<id>']."
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
        # Death-watch state: `_death` is the rendered cause while the kernel is
        # down (entry guard), `_death_event` fails calls already in flight, and
        # `_watch_task` is the poll loop itself (cancelled by shutdown()).
        self._death: str | None = None
        self._death_event = asyncio.Event()
        self._watch_task: asyncio.Task | None = None
        self._restore_task: asyncio.Task | None = None
        # The session identity the server last pushed (client label, session
        # name, topic), re-applied by restart() so a respawned kernel keeps the
        # dashboard grouping the client already chose instead of resetting to
        # the default label.
        self._client_label: str | None = None
        self._session_name: str | None = None
        self._session_topic: str | None = None
        # Entry guard for restart_now(): two concurrent intentional restarts
        # would cancel each other's death watch and double-respawn.
        self._restarting = False

    async def start(self) -> None:
        from jupyter_client.manager import AsyncKernelManager

        # Point the kernel's faulthandler at a private file before launch; the
        # kernel inherits this env and registers the SIGUSR1 dump handler. The
        # name carries this server's pid: every serve on the machine shares
        # runtime_dir(), and one fixed name had concurrent kernels truncating
        # and interleaving each other's dumps, so kernel_trace could return a
        # different session's stacks (index#2355).
        self._trace_path = trace_path_for(os.getpid())
        os.environ[TRACE_ENV] = str(self._trace_path)
        _sweep_stale_traces()

        self._km = AsyncKernelManager(kernel_name="python3")
        await self._km.start_kernel(cwd=str(self._config.workdir))
        self._pid = self._kernel_pid()
        self._kc = self._km.client()
        self._kc.start_channels()
        await self._kc.wait_for_ready(timeout=_READY_TIMEOUT)
        # Watch the child we just spawned so an external kill is noticed the
        # moment it happens, not at the next execute's timeout (index#2339).
        self._watch_task = asyncio.ensure_future(self._watch())

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
        if self._death is not None:
            return f"kernel process is gone: {self._death}"
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
                # The signal may have gone to a zombie (a killed child not yet
                # reaped): os.kill succeeds but nothing can ever dump. The death
                # watch reaps and flags it within its poll interval; report the
                # death instead of waiting out the deadline (index#2339).
                if self._death is not None:
                    return f"kernel process is gone: {self._death}"
                if path.exists() and path.stat().st_size > before:
                    # A short settle so the whole multi-thread dump has flushed.
                    await asyncio.sleep(0.05)
                    return path.read_text()[before:].strip() or "(empty trace)"
        # No dump and no flagged death: distinguish a gone process (say so
        # plainly) from a live kernel that cannot service signals.
        if self._death is not None or not await self._km.is_alive():
            return "kernel process is gone: " + (self._death or f"kernel died (pid {self._pid})")
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
            # Race the reply against kernel death: a request to a process that
            # exits mid-cell never gets a reply, so waiting out the full
            # ``timeout`` would misreport an external kill as a wedge AND hold
            # this lock against the respawn (index#2339). The death watch sets
            # the event the moment the child exits.
            died = asyncio.ensure_future(self._death_event.wait())
            try:
                try:
                    await asyncio.shield(asyncio.wait({task, died}, return_when=asyncio.FIRST_COMPLETED))
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
                if not task.done():
                    # The kernel died with this request in flight: the reply can
                    # never arrive. The process is gone and the respawn rebuilds
                    # the channels, so there is no half-read reply to protect;
                    # drop the read and surface the precise cause.
                    task.cancel()
                    with contextlib.suppress(BaseException):
                        await task
                    raise KernelDiedError(self._death or "kernel died; respawning")
                await task  # propagate the reply's own error (e.g. TimeoutError)
            finally:
                died.cancel()
            return outputs, summary

    async def python_exec(
        self,
        code: str,
        budget: float,
        name: str | None = None,
        session: str | None = None,
        topic: str | None = None,
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
        the kernel, verify with a probe that the interrupt actually landed, and
        escalate to a kernel restart when it did not (index#2375: a block inside
        native code never reaches the Python-level rescue handler, so delivery
        alone recovers nothing) -- then return an actionable summary instead of
        letting an opaque ``Timeout waiting for output`` escape to the caller.
        """
        name_arg = "None" if name is None else repr(name)
        session_arg = "None" if session is None else repr(session)
        topic_arg = "None" if topic is None else repr(topic)
        wrapper = (
            f"await __ix_exec({code!r}, budget={float(budget)!r}, "
            f"name={name_arg}, session={session_arg}, topic={topic_arg})"
        )
        self._check_alive()
        grace = self._config.wedge_grace
        deadline = float(budget) + grace
        try:
            return await self._execute(wrapper, timeout=deadline)
        except TimeoutError:
            # A dead kernel also never replies: if the death watch flagged one
            # while this request waited, report that precise cause, never a wedge.
            self._check_alive()
            interrupted = await self._interrupt()
            if interrupted and await self._probe_idle():
                outcome = "recovered"
            else:
                # Delivery is not recovery: the SIGUSR2 rescue is a Python-level
                # handler that only runs at a bytecode boundary, so a main thread
                # blocked inside native code (the embedded nu engine, index#2095;
                # any C extension) never executes it. Left alone the kernel wedges
                # until the client SIGTERMs the whole serve (index#2365), so kill
                # and respawn just the kernel child instead (index#2375). Safe:
                # session restore replays only SUCCESSFUL cells, so the wedged
                # cell is not re-run.
                #
                # First preserve the evidence the kill would destroy: faulthandler's
                # SIGUSR1 dump is C-level, so it fires even mid-native-block and
                # names the frame that called into it -- exactly the stack #2095
                # needs from a live occurrence. Best-effort, to stderr/journald.
                with contextlib.suppress(Exception):  # evidence capture must never block the escalation
                    trace = await self.dump_trace()
                    print(
                        f"[ix-mcp] wedged kernel stack before escalation kill (pid {self._pid}):\n{trace}",
                        file=sys.stderr,
                        flush=True,
                    )
                try:
                    await self.restart_now(freshen=False, reason="wedge escalation, index#2375")
                    outcome = "restarted"
                except RuntimeError:
                    # Another call's escalation (or an operator's kernel_restart)
                    # already owns the respawn; its restore will serve us too.
                    outcome = "restart_pending"
            return [], _wedged_summary(budget, grace, deadline, outcome=outcome)

    async def cancel_running(self, session: str | None) -> list[str]:
        """Cancel the run this ``session``'s abandoned ``python_exec`` launched.

        The server calls this when a client cancels an in-flight ``python_exec``
        (``notifications/cancelled`` or a transport abort): the tool coroutine is
        cancelled server-side, but the job it started keeps running in the kernel
        as a background task, executing side effects the caller already abandoned
        (index#2387). This pokes ``__ix_cancel_running`` on the raw shell channel
        (no job/card, like ``set_client``), which cancels the same job an explicit
        ``jobs['<id>'].cancel()`` would. Best-effort: a cancel arriving after the
        run finished (the common race) cancels nothing, and a failure here must
        never mask the original cancellation the caller is propagating. Returns
        the ids cancelled (parsed from the raw reply; empty on any hiccup)."""
        if self._pid is None:
            return []
        session_arg = "None" if session is None else repr(session)
        try:
            outputs, _ = await self._execute(
                f"print(__ix_cancel_running(session={session_arg}))", timeout=10.0
            )
        except BaseException:  # cancel is best-effort; never mask the caller's own cancellation
            return []
        text = "".join(
            o.get("text", "") for o in outputs if isinstance(o, dict)
        ).strip()
        # `print(list)` renders `['ab12']`; parse it back defensively so a
        # malformed reply degrades to "cancelled nothing" rather than raising.
        with contextlib.suppress(Exception):
            value = ast.literal_eval(text)
            if isinstance(value, list):
                return [str(item) for item in value]
        return []

    async def set_client(self, client: str) -> None:
        """Tell the kernel which MCP client connected, so the session label can
        default to it. Runs as a raw shell request (not ``__ix_exec``), so it
        leaves no job/card behind — it only pokes ``session._set_client``. The
        server calls this once, when the client identifies itself."""
        self._client_label = client
        with contextlib.suppress(Exception):  # session label is a convenience; must not break the tool call
            await self._execute(f"session._set_client({client!r})", timeout=10.0)

    async def set_session_name(self, name: str) -> None:
        """Set the dashboard session label without creating an execution card.

        This is the MCP-side naming handshake, not user code, so it uses the raw
        shell channel instead of ``__ix_exec``.
        """
        self._check_alive()
        await self._execute(f"session.name = {name!r}\nsession._sync()", timeout=10.0)
        self._session_name = name

    async def set_topic(self, topic: str) -> None:
        """Set the dashboard topic without creating an execution card."""
        self._check_alive()
        await self._execute(f"session.topic = {topic!r}", timeout=10.0)
        self._session_topic = topic

    async def _reapply_session(self) -> None:
        """Re-push the session identity the server already set (client label,
        session name, topic) into a fresh kernel process. The respawned kernel
        boots with a default ``Session`` and nothing else replays these -- the
        checkpoint covers user-bound names only -- so without this a restart
        silently resets the dashboard grouping the client already chose.
        Best-effort, like ``set_client``: a label must never fail a restart."""
        with contextlib.suppress(Exception):  # labels are a convenience; the restart must proceed without them
            if self._client_label is not None:
                await self._execute(f"session._set_client({self._client_label!r})", timeout=10.0)
            if self._session_name is not None:
                await self._execute(f"session.name = {self._session_name!r}\nsession._sync()", timeout=10.0)
            if self._session_topic is not None:
                await self._execute(f"session.topic = {self._session_topic!r}", timeout=10.0)

    async def _interrupt(self) -> bool:
        """Break a synchronous call wedging the kernel's event loop. ipykernel's
        own ``interrupt_kernel`` cancels the asyncio task, which a synchronous call
        never yields to, so it cannot break a wedged async cell. Send SIGUSR2 to
        the kernel's runtime handler instead: it raises ``KeyboardInterrupt`` inline
        at the blocked frame, which ``_runner`` records as a failed job so the
        kernel returns to idle and the next call runs. Returns whether the signal
        was DELIVERED -- not whether it recovered anything: the handler is
        Python-level, so a main thread blocked inside native code never runs it
        (index#2375). Callers must verify with :meth:`_probe_idle`."""
        if self._pid is None:
            return False
        try:
            os.kill(self._pid, signal.SIGUSR2)
        except ProcessLookupError:
            return False
        return True

    async def _probe_idle(self, timeout: float | None = None) -> bool:
        """Whether the kernel answers a trivial execute, i.e. a rescue attempt
        actually returned it to idle. ``wait_for`` also bounds the wait for the
        shell lock itself, which an unrescued kernel's earlier request may still
        hold; cancelling ``_execute`` there is safe (its cancel path drains the
        socket under the lock before re-raising)."""
        budget = timeout if timeout is not None else max(5.0, self._config.wedge_grace)
        try:
            await asyncio.wait_for(self._execute("pass", timeout=budget), timeout=budget + 5.0)
        except TimeoutError:
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

    async def snapshot_session(self, timeout: float = 60.0) -> None:
        """Best-effort final checkpoint (shutdown, or just before an intentional
        restart), so the last cells' state is in the file even if the debounced
        checkpoint had not fired yet. ``timeout`` bounds the wait: a caller
        about to kill a possibly-wedged kernel keeps it short."""
        with contextlib.suppress(Exception):  # the caller must proceed; periodic checkpoint plus replay guarantee a correct reopen
            await self._execute("await __ix_snapshot()", timeout=timeout)

    async def emit_read_stats_final(self) -> None:
        """Flush the final ``mcp_read_stats`` line per session at shutdown, so the
        counts since the last periodic (~300s) emit reach the journal. Must run
        BEFORE ``shutdown()`` (which kills the kernel with SIGKILL, past which no
        in-kernel or atexit code runs). Best-effort: a flush failure must not block
        shutdown, and the periodic emit already covered every prior window."""
        with contextlib.suppress(Exception):  # shutdown must proceed even if the final flush fails
            await self._execute("__ix_emit_read_stats_final()", timeout=30.0)

    def _check_alive(self) -> None:
        """Fail a tool entry point with the precise death cause while the kernel
        is down: `kernel died (pid N, signal S); respawning`, never a generic
        wedge or transport timeout against a process that no longer exists."""
        if self._death is not None:
            raise KernelDiedError(self._death)

    def _exit_code(self) -> int | None:
        """The dead kernel's returncode (negative: killed by that signal), read
        from the Popen the provisioner's liveness poll reaped."""
        provisioner = getattr(self._km, "provisioner", None)
        process = getattr(provisioner, "process", None)
        code = getattr(process, "returncode", None)
        return code if isinstance(code, int) else None

    async def _watch(self) -> None:
        """Notice the kernel child exiting the moment it happens.

        Without this watch an externally killed kernel (a stray `pkill -f
        ipykernel_launcher`, the OOM killer, a crash) presented as a generic
        'wedged' timeout, `kernel_trace` signalled the zombie and blamed a
        missing faulthandler, and the kernel only came back on the next execute
        (index#2339). On death: record the precise cause, fail in-flight and
        subsequent calls with it, say it loudly on stderr (which reaches
        journald), and respawn immediately."""
        while True:
            await asyncio.sleep(_WATCH_INTERVAL)
            km = self._km
            if km is None:
                return
            try:
                alive = await km.is_alive()
            except Exception:  # noqa: S112 -- a transient introspection error must not end the lifetime watch
                continue
            if alive:
                continue
            pid = self._pid
            cause = _describe_exit(self._exit_code())
            self._death = f"kernel died (pid {pid}, {cause}); respawning"
            self._death_event.set()
            print(f"[ix-mcp] {self._death}", file=sys.stderr, flush=True)
            delay = 1.0
            while True:
                try:
                    await self.restart()
                    break
                except Exception as exc:
                    # Still loud, still precise; keep trying so the server heals
                    # without waiting for a client call.
                    self._death = f"kernel died (pid {pid}, {cause}); respawn failed: {exc!r}, retrying in {delay:.0f}s"
                    print(f"[ix-mcp] {self._death}", file=sys.stderr, flush=True)
                    await asyncio.sleep(delay)
                    delay = min(delay * 2, 30.0)
            print(
                f"[ix-mcp] kernel respawned (pid {self._pid}) after pid {pid} died ({cause})",
                file=sys.stderr,
                flush=True,
            )

    async def restart(self) -> None:
        """Start a fresh kernel process; the death watch's respawn primitive.

        Rebuilds everything the old process owned: the pid the trace/interrupt
        signals target (a stale pid would signal a dead or reused process), the
        client channels (the old sockets may hold a dead kernel's half-delivered
        replies), and -- exactly as `serve` does at startup -- the session
        checkpoint, restored while holding the shell channel so every call
        admitted once the death flag clears queues behind the restored state.
        The cwd is re-passed fresh so a respawn survives the original launch
        directory having been deleted since (index#2120). The new kernel re-runs
        install() and re-opens the trace file.
        """
        if self._km is None:
            return
        async with self._lock:
            await self._km.restart_kernel(now=True, cwd=str(self._config.workdir))
            self._pid = self._kernel_pid()
            if self._kc is not None:
                self._kc.stop_channels()
            self._kc = self._km.client()
            self._kc.start_channels()
            await self._kc.wait_for_ready(timeout=_READY_TIMEOUT)
        # The new process is alive: in-flight racing stops now (fresh event); the
        # entry guard (`_death`) stays up until the restore holds the channel.
        self._death_event = asyncio.Event()
        # Re-push the session identity the server already set, BEFORE the
        # restore, so replayed cells group under the label the client chose.
        await self._reapply_session()
        if self._config.session_resume:
            locked = asyncio.Event()

            async def _restore() -> None:
                try:
                    summary = await self.restore_session(on_locked=locked.set)
                    if summary:
                        print(f"[ix-mcp] {summary}", file=sys.stderr, flush=True)
                except Exception as exc:
                    print(f"[ix-mcp] session restore after respawn failed: {exc!r}", file=sys.stderr, flush=True)
                finally:
                    locked.set()  # a restore that died before locking must not hold the death flag forever

            self._restore_task = asyncio.ensure_future(_restore())
            await locked.wait()
        self._death = None

    async def restart_now(
        self, *, freshen: bool = True, reason: str = "requested via kernel_restart"
    ) -> dict[str, int | float | None]:
        """An intentional restart of this server's kernel: the `kernel_restart`
        tool's primitive (index#2345) and the wedge escalation's (index#2375).

        Reuses the death watch's respawn machinery (``restart()``: fresh
        process, rebuilt channels, checkpoint restore, session labels
        re-applied) but for a kill made ON PURPOSE, so the watch is cancelled
        first -- exactly the order ``shutdown()`` uses -- and must neither
        report this as a death nor race its own respawn against it; it is
        re-armed for the new process afterwards. Loud on stderr (which reaches
        journald), like the death watch, so an operator can see who bounced a
        kernel and when. ``freshen=False`` skips the pre-kill checkpoint
        freshening: the wedge escalation calls this against a kernel known to be
        blocked, where the snapshot attempt would only burn its timeout.
        Returns the old pid, the new pid, and the elapsed seconds.
        """
        if self._km is None:
            raise RuntimeError("the kernel is not running")
        if self._restarting:
            raise RuntimeError("a kernel restart is already in progress")
        self._restarting = True
        try:
            loop = asyncio.get_running_loop()
            started = loop.time()
            old_pid = self._pid
            print(f"[ix-mcp] kernel restart requested (pid {old_pid}; {reason})", file=sys.stderr, flush=True)
            # Stop the death watch FIRST, exactly as shutdown() does: the kill
            # below is intentional.
            if self._watch_task is not None:
                self._watch_task.cancel()
                with contextlib.suppress(asyncio.CancelledError):
                    await self._watch_task
                self._watch_task = None
            # Freshen the checkpoint so the restore replays (i.e. RE-EXECUTES)
            # as little as possible -- but on a short leash: the kernel may be
            # wedged, the very reason for this restart, and the periodic
            # checkpoint plus replay already guarantee a correct reopen.
            if freshen and self._death is None and self._config.session_resume:
                with contextlib.suppress(Exception):  # best-effort freshness; a timeout here means a wedged kernel, which the restart below fixes
                    await asyncio.wait_for(self.snapshot_session(timeout=10.0), timeout=15.0)
            # Fail calls already in flight (a wedged cell would otherwise hold
            # the shell lock against the respawn for its whole budget) and guard
            # new ones until the restore holds the channel -- the death watch's
            # own mechanism, with the honest cause instead of a death report.
            self._death = f"kernel is restarting ({reason}; was pid {old_pid}); retry shortly"
            self._death_event.set()
            await self.restart()
            self._watch_task = asyncio.ensure_future(self._watch())
            elapsed = loop.time() - started
            print(
                f"[ix-mcp] kernel restarted on request (pid {old_pid} -> {self._pid}) in {elapsed:.1f}s",
                file=sys.stderr,
                flush=True,
            )
            return {"old_pid": old_pid, "new_pid": self._pid, "elapsed_s": round(elapsed, 2)}
        finally:
            self._restarting = False

    async def shutdown(self) -> None:
        # Stop the death watch first: shutdown kills the kernel on purpose, and
        # the watch must not report that as a death or respawn over it.
        if self._watch_task is not None:
            self._watch_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._watch_task
            self._watch_task = None
        if self._kc is not None:
            self._kc.stop_channels()
        if self._km is not None:
            await self._km.shutdown_kernel(now=True)
        # This serve owns its trace file (the name carries our pid): remove it
        # so clean exits leave nothing behind; SIGKILLed serves are covered by
        # the sweep at the next start().
        if self._trace_path is not None:
            self._trace_path.unlink(missing_ok=True)
            self._trace_path = None


_KERNEL: Kernel | None = None


def set_kernel(kernel: Kernel) -> None:
    global _KERNEL
    _KERNEL = kernel


def current_kernel() -> Kernel:
    if _KERNEL is None:
        raise RuntimeError("the kernel is not running; call a tool inside `ix-mcp serve`")
    return _KERNEL
