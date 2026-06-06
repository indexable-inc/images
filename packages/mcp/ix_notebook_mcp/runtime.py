"""Kernel-side runtime: the part that runs *inside* the ipykernel.

It is loaded once per kernel by the shipped IPython startup script
(``ipython/00-ix-runtime.py``) calling :func:`install`. After that the user
namespace has:

  - ``jobs``  : a dict of every execution this kernel has run (the registry the
    agent manipulates with more ``python_exec`` calls: ``jobs['ab12'].cancel()``,
    ``await jobs['ab12']``, ``[j for j in jobs.values() if j.running()]``).
  - ``Job``   : the handle type (status, output, result, ``.cancel()``, awaitable).
  - ``__ix_exec(code, budget)`` : what the MCP server calls per ``python_exec``.

Concurrency model (validated): each execution runs as an asyncio task on the
kernel's own event loop, so many run at once and none blocks the others. Per-job
stdout/stderr is captured by routing writes through a ``ContextVar`` set inside
each task, so interleaved prints land in the right job. A blocking call (fff,
numpy, a subprocess) stays non-blocking by going through ``asyncio.to_thread``
(its GIL-releasing native work then runs off the loop).

Every job also writes itself to the SQLite store at ``IX_MCP_STORE`` (start, a
throttled output tail while running, final status) so the dashboard can show all
running things and their live output without ever touching the kernel. The
result's rich display (HTML tables, images) and any ``display()`` calls made
while a job runs are captured into the store too, so the dashboard renders them
like a notebook instead of plain text.
"""

from __future__ import annotations

import ast
import asyncio
import base64
import contextvars
import inspect
import json
import os
import sys
import time
import traceback
import uuid

_ix_current: contextvars.ContextVar = contextvars.ContextVar("ix_current_job", default=None)

# Cap on a single job's captured output kept in memory (and mirrored to the store
# and the dashboard). A chatty/runaway job keeps only the most recent slice, so
# memory, store writes, and poll payloads all stay bounded.
_MAX_OUTPUT_CHARS = 256_000

# The custom mime the kernel hands the server to carry a job summary (mirrors
# outputs.JOB_MIME; duplicated so the kernel-side runtime stays import-light).
JOB_MIME = "application/x-ix-job+json"

# Rich display capture: which mimes we keep for the dashboard, and per-mime size
# caps. Truncating a base64 image yields a corrupt data URI, so an oversize image
# is dropped whole rather than clipped; text mimes clip with a marker.
_RICH_MIMES = (
    "text/html",
    "image/png",
    "image/jpeg",
    "image/svg+xml",
    "text/markdown",
    "application/json",
    "text/plain",
)
_IMAGE_MIMES = frozenset({"image/png", "image/jpeg"})
_MAX_TEXT_BUNDLE = 400_000
_MAX_IMAGE_BUNDLE = 4_000_000

# Opened lazily in install(); None when no store path is configured (the
# one-shot eval/exec paths, or a bare kernel started outside the server).
_store_conn = None
_store = None
_shell = None  # the InteractiveShell, set in install(); used to format rich results


class _Tee:
    """sys.stdout/err replacement that routes each write to the *current task's*
    job buffer (so concurrent jobs keep separate output) plus the real stream."""

    def __init__(self, original):
        self._original = original

    def write(self, s):
        job = _ix_current.get()
        if job is not None:
            # Job output is captured here (and persisted to the store) rather than
            # streamed to IOPub, so the server reads it back from the job summary
            # and a backgrounded job's output is not lost to a stale cell context.
            job._append(s)
            return len(s)
        return self._original.write(s)

    def flush(self):
        try:
            self._original.flush()
        except Exception:
            # Flush failures on the wrapped kernel stream are non-fatal.
            pass

    def __getattr__(self, name):
        return getattr(self._original, name)


class Job:
    """A single ``python_exec`` execution: an awaitable handle over the asyncio
    task running the code, with its captured output, result, and status."""

    def __init__(self, code: str, name: str | None = None):
        self.id = uuid.uuid4().hex[:8]
        self.code = code
        self.name = name or self.id
        self.status = "running"
        self.started = time.time()
        self.ended: float | None = None
        self.result = None
        self.error: str | None = None
        self._buf: list[str] = []
        self._buflen = 0
        # Rich outputs (mime bundles) display()-ed while this job runs.
        self._displays: list[dict] = []
        self.task: asyncio.Task | None = None

    def _append(self, s: str) -> None:
        """Append output, trimming to the most recent _MAX_OUTPUT_CHARS so a
        runaway job cannot grow the buffer (or the store row / poll payload)
        without bound."""
        self._buf.append(s)
        self._buflen += len(s)
        if self._buflen > _MAX_OUTPUT_CHARS:
            kept = "".join(self._buf)[-_MAX_OUTPUT_CHARS:]
            self._buf = [kept]
            self._buflen = len(kept)

    @property
    def output(self) -> str:
        return "".join(self._buf)

    def tail(self, n: int = 2000) -> str:
        return self.output[-n:]

    def running(self) -> bool:
        return self.status == "running"

    def cancel(self) -> "Job":
        if self.task is not None and not self.task.done():
            self.task.cancel()
        return self

    def __await__(self):
        # `await jobs['id']` should yield the job's result, but the runner task
        # returns None, so wait for it then hand back the captured result.
        async def _await_result():
            await self.task
            return self.result

        return _await_result().__await__()

    def __repr__(self) -> str:
        dur = (self.ended or time.time()) - self.started
        head = f"<Job {self.id} ({self.name}) [{self.status}] {dur:.2f}s>"
        out = self.tail(800)
        return head + ("\n" + out if out else "")


jobs: dict[str, Job] = {}


def _compile(code: str, filename: str):
    """Compile statements with top-level ``await`` allowed, capturing the value
    of a trailing expression into ``__ix_result`` (REPL-style), so a job has a
    result like a notebook cell does."""
    tree = ast.parse(code, filename, "exec")
    if tree.body and isinstance(tree.body[-1], ast.Expr):
        last = tree.body[-1]
        assign = ast.Assign(targets=[ast.Name(id="__ix_result", ctx=ast.Store())], value=last.value)
        ast.copy_location(assign, last)
        tree.body[-1] = assign
        ast.fix_missing_locations(tree)
    return compile(tree, filename, "exec", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)


async def _runner(job: Job, ns: dict) -> None:
    token = _ix_current.set(job)
    if _store is not None and _store_conn is not None:
        try:
            _store.start(_store_conn, id=job.id, name=job.name, code=job.code, started_at=job.started)
        except Exception:
            # Best-effort logging: a store write must never abort the job.
            pass
    try:
        # Compile inside the runner so a SyntaxError is recorded as a job error
        # (status + traceback in the store/dashboard) instead of escaping __ix_run.
        code_obj = _compile(job.code, f"<job {job.id}>")
        ns.pop("__ix_result", None)
        maybe = eval(code_obj, ns)
        if inspect.iscoroutine(maybe):
            await maybe
        job.result = ns.pop("__ix_result", None)
        job.status = "done"
    except asyncio.CancelledError:
        job.status = "cancelled"
        raise
    except (Exception, SystemExit, KeyboardInterrupt):
        # Isolate user code from the kernel: a job's SyntaxError, exception, or
        # even sys.exit()/exit() becomes a failed job (traceback captured) instead
        # of escaping the task and tearing down the shared kernel session.
        # asyncio.CancelledError is BaseException, not caught here, so cooperative
        # cancellation (handled above) still propagates.
        job.status = "error"
        job.error = traceback.format_exc()
        job._append(job.error)
    finally:
        job.ended = time.time()
        _ix_current.reset(token)
        _persist_final(job)


def _persist_final(job: Job) -> None:
    if _store is None or _store_conn is None:
        return
    try:
        result_repr = None if job.result is None else _safe_repr(job.result)
        _store.finish(
            _store_conn,
            id=job.id,
            status=job.status,
            ended_at=job.ended or time.time(),
            output=job.output,
            result=result_repr,
            error=job.error,
            outputs=_job_outputs(job),
        )
    except Exception:
        # Best-effort logging: persisting the final status must not raise during cleanup.
        pass


def _safe_repr(value) -> str:
    try:
        return repr(value)
    except Exception:
        return f"<unreprable {type(value).__name__}>"


def _normalize_bundle(data: dict, metadata: dict | None = None) -> dict:
    """Coerce a display formatter mime bundle to JSON-safe values (bytes -> base64),
    keeping only whitelisted mimes within size caps, for the store and dashboard."""
    out: dict[str, str] = {}
    for mime in _RICH_MIMES:
        if mime not in data:
            continue
        value = data[mime]
        if isinstance(value, (bytes, bytearray)):
            value = base64.b64encode(bytes(value)).decode("ascii")
        elif not isinstance(value, str):
            try:
                value = json.dumps(value)
            except Exception:
                value = str(value)
        if mime in _IMAGE_MIMES:
            if len(value) > _MAX_IMAGE_BUNDLE:
                continue  # clipping a base64 image corrupts the data URI; drop it
        elif len(value) > _MAX_TEXT_BUNDLE:
            value = value[:_MAX_TEXT_BUNDLE] + "\n... [truncated]"
        out[mime] = value
    return {"data": out, "metadata": metadata or {}}


def _result_bundle(value) -> dict | None:
    """Render a job's result through IPython's display machinery (a polars
    DataFrame yields text/html, a matplotlib Figure image/png) for the dashboard."""
    if _shell is None:
        return None
    try:
        data, metadata = _shell.display_formatter.format(value)
    except Exception:
        return None
    bundle = _normalize_bundle(data, metadata)
    return bundle if bundle["data"] else None


def _job_outputs(job: "Job") -> list[dict]:
    """A job's rich outputs for the store: every display() bundle captured while it
    ran, plus the trailing-expression result rendered the same way."""
    outs = list(job._displays)
    if job.result is not None and not job.running():
        bundle = _result_bundle(job.result)
        if bundle is not None:
            outs.append(bundle)
    return outs


async def _flusher() -> None:
    """Throttled background loop: persist every running job's output tail to the
    store so the dashboard shows live output. One loop for all jobs (cheap)."""
    if _store is None or _store_conn is None:
        return
    while True:
        await asyncio.sleep(0.5)
        for job in list(jobs.values()):
            if job.running():
                try:
                    _store.update_output(_store_conn, job.id, job.output, job._displays or None)
                except Exception:
                    # Best-effort live output: a store write must not kill the loop.
                    pass


async def __ix_run(code: str, budget: float = 15.0, name: str | None = None) -> Job:
    """Run ``code`` as a task; wait up to ``budget`` for it; return the Job either
    way (done, or still running in the background)."""
    ns = _user_ns if _user_ns is not None else globals()
    job = Job(code, name)
    jobs[job.id] = job
    job.task = asyncio.ensure_future(_runner(job, ns))
    await asyncio.wait({job.task}, timeout=budget)
    return job


def _emit(job: Job) -> None:
    """Publish a structured summary the MCP server parses, plus the result's rich
    repr (image/HTML/table) as normal display output the server already renders."""
    from IPython.display import display, publish_display_data

    summary = {
        "id": job.id,
        "name": job.name,
        "status": job.status,
        "running": job.running(),
        "output": job.tail(50_000),
        "result": None if job.result is None else _safe_repr(job.result),
        "error": job.error,
    }
    publish_display_data({JOB_MIME: summary, "text/plain": f"[{job.id}] {job.status}"})
    if job.result is not None and not job.running():
        try:
            display(job.result)
        except Exception:
            # Rich display is best-effort; failures must not block the summary.
            pass


async def __ix_exec(code: str, budget: float = 15.0, name: str | None = None) -> None:
    """The MCP server's per-call entrypoint: run with a budget, emit the summary."""
    job = await __ix_run(code, budget=budget, name=name)
    _emit(job)


def _install_display_capture(shell) -> None:
    """Route display() / rich auto-display made *inside a job* to that job's output
    list (still forwarding to IOPub for the agent's reply), so the dashboard can
    show images and HTML tables, not just text."""
    pub = shell.display_pub
    if getattr(pub, "_ix_wrapped", False):
        return
    original = pub.publish

    def publish(data, metadata=None, **kwargs):
        job = _ix_current.get()
        if job is not None and isinstance(data, dict) and JOB_MIME not in data:
            bundle = _normalize_bundle(data, metadata)
            if bundle["data"]:
                job._displays.append(bundle)
        return original(data, metadata, **kwargs)

    pub.publish = publish
    pub._ix_wrapped = True


def _figure_png(fig) -> bytes:
    import io

    buf = io.BytesIO()
    fig.savefig(buf, format="png", bbox_inches="tight")
    return buf.getvalue()


def _register_rich_formatters(shell) -> None:
    """Make matplotlib figures render as image/png. A bare ipykernel only wires the
    inline png formatter after %matplotlib inline; register it lazily by type name
    so importing matplotlib stays the user's choice."""
    try:
        png = shell.display_formatter.formatters["image/png"]
        png.for_type_by_name("matplotlib.figure", "Figure", _figure_png)
    except Exception:
        # No display formatter (non-IPython host) or a matplotlib too old to wire.
        pass


_user_ns: dict | None = None


def install(user_ns: dict | None = None) -> None:
    """Wire the runtime into the kernel: tee stdout/err, open the store, start the
    flusher, and expose the registry + entrypoints in the user namespace."""
    global _store, _store_conn, _user_ns, _shell
    _user_ns = user_ns

    if not isinstance(sys.stdout, _Tee):
        sys.stdout = _Tee(sys.stdout)
    if not isinstance(sys.stderr, _Tee):
        sys.stderr = _Tee(sys.stderr)

    import IPython

    _shell = IPython.get_ipython()
    if _shell is not None:
        # Capture rich display output and teach the kernel to render figures, so
        # the dashboard shows tables/images like a notebook (not just text).
        _install_display_capture(_shell)
        _register_rich_formatters(_shell)

    store_path = os.environ.get("IX_MCP_STORE")
    if store_path:
        try:
            from . import store as _store_mod

            _store = _store_mod
            _store_conn = _store_mod.connect(store_path)
        except Exception:
            # A broken store must not stop code execution; jobs just are not logged.
            _store = None
            _store_conn = None

    target = user_ns if user_ns is not None else globals()
    target["jobs"] = jobs
    target["Job"] = Job
    target["__ix_run"] = __ix_run
    target["__ix_exec"] = __ix_exec
    target["DASHBOARD_URL"] = os.environ.get("IX_MCP_DASHBOARD_URL", "")

    try:
        asyncio.get_event_loop().create_task(_flusher())
    except RuntimeError:
        # No loop yet (e.g. install from a sync context): the flusher is optional.
        pass
