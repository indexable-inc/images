# ruff: noqa: ANN401 -- runtime handles arbitrary Python objects; Any is the correct type throughout
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
each task, so interleaved prints land in the right job. A blocking call (numpy,
a subprocess) stays non-blocking by going through ``asyncio.to_thread``
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
import binascii
import collections
import contextlib
import contextvars
import datetime
import dataclasses
import html as html_lib
import inspect
import json
import os
import pathlib
import re
import secrets
import signal
import sys
import time
import traceback
import types
import uuid
from collections.abc import Awaitable, Callable, Mapping
from typing import TYPE_CHECKING, Any, overload

from . import readstats, registry, typecheck
from .config import build_stamp, process_cwd

_ix_current: contextvars.ContextVar = contextvars.ContextVar("ix_current_job", default=None)

# Cap on a single job's captured output kept in memory (and mirrored to the store
# and the dashboard). A chatty/runaway job keeps only the most recent slice, so
# memory, store writes, and poll payloads all stay bounded.
_MAX_OUTPUT_CHARS = 256_000

# Bounded record of background-task failures, newest last; bound into the user
# namespace as `task_errors`. asyncio only reports a failed, never-awaited task
# at garbage collection, and never reports one a namespace variable keeps alive
# -- which is exactly the fire-and-forget watcher pattern this kernel invites
# (`asyncio.create_task(...)` bound to a name). One such watcher died on an
# AttributeError on 2026-07-02 and starved its monitors for 90 minutes with no
# trace anywhere. The task factory below reports at completion instead.
task_errors: collections.deque[str] = collections.deque(maxlen=50)

# The namespace-install flusher task (see _install), module-held so it cannot
# be garbage-collected mid-flight.
_flusher_task: asyncio.Task | None = None

# Grace period between a task finishing with an exception and the failure being
# reported: a parent that promptly retrieves it (`await task`, `gather`,
# `.exception()`) wins the race and nothing is reported; a task nobody checks
# within this window is treated as fire-and-forget and surfaced. A parent that
# retrieves *later* still gets the exception raised normally -- it just also
# left a (harmless) report behind.
_TASK_FAILURE_GRACE_S = 2.0


def _report_task_failure(task: asyncio.Task) -> None:
    exc = task.exception()  # also clears the interpreter's own GC-time warning, which this report replaces
    if exc is None:
        return
    tb = "".join(traceback.format_exception(type(exc), exc, exc.__traceback__))
    coro = task.get_coro()
    where = getattr(coro, "__qualname__", None) or repr(coro)
    # "unretrieved after Ns", not "crashed": a parent that awaits this task
    # later than the grace window still gets the exception raised normally --
    # this report then just flagged it early, it is not a second failure.
    msg = (
        f"[task_errors] background task {task.get_name()!r} ({where}) failed and nothing "
        f"retrieved the exception within {_TASK_FAILURE_GRACE_S:g}s (a later `await` "
        f"still raises it):\n{tb}"
    )
    task_errors.append(msg)
    # Tasks copy the spawning cell's context, so the job that created this task
    # is reachable and its buffer is where the model will look first.
    job = task.get_context().get(_ix_current)
    if job is not None:
        job._append(msg)
        # The common fire-and-forget case dies AFTER its spawning cell
        # finished, i.e. after _persist_final already wrote the store row; a
        # bare _append would then be visible to `jobs[id].output` but never
        # reach the dashboard card. Re-persist so the stored row carries it.
        if not job.running():
            _persist_final(job)
    with contextlib.suppress(Exception):  # reporting must never take the loop down
        print(msg, file=sys.__stderr__, flush=True)


def _on_task_done(task: asyncio.Task) -> None:
    if task.cancelled():
        return

    def check() -> None:
        # `_log_traceback` is the interpreter's own "exception not yet
        # retrieved" flag (cleared by `await`/`.result()`/`.exception()`);
        # private, but it is precisely the signal CPython's GC-time warning
        # keys on, and there is no public completion-time equivalent.
        if getattr(task, "_log_traceback", False):
            _report_task_failure(task)

    with contextlib.suppress(RuntimeError):  # loop already closed: nowhere left to report
        task.get_loop().call_later(_TASK_FAILURE_GRACE_S, check)


def _install_task_failure_watch(loop: asyncio.AbstractEventLoop) -> None:
    """Report every task that finishes with an unretrieved exception (see
    `task_errors`). Installed as a task factory because a done-callback is the
    only completion-time hook asyncio offers, and wrapping the factory is the
    only way to attach one to every task, including ones third-party code
    spawns. Idempotent: re-running `install()` on the same loop must not stack
    watchers (each stack would double the per-task callback and 2s timer)."""
    if getattr(loop.get_task_factory(), "_ix_task_watch", False):
        return
    prior = loop.get_task_factory()

    def factory(loop: asyncio.AbstractEventLoop, coro: Any, **kwargs: Any) -> asyncio.Task:
        task = prior(loop, coro, **kwargs) if prior is not None else asyncio.Task(coro, loop=loop, **kwargs)
        task.add_done_callback(_on_task_done)
        return task

    factory._ix_task_watch = True  # our own sentinel, read back by the guard above
    loop.set_task_factory(factory)

# Longest edge (px) of an image returned to the model. A full-page screenshot or
# hi-DPI figure otherwise spends vision tokens scaling with its resolution for no
# added legibility, so an oversize raster image is downscaled (aspect preserved,
# re-encoded as PNG) before it is base64-encoded into the reply. Set
# ``IX_MCP_IMAGE_MAX_DIM=0`` to disable downscaling and send images at full size.
try:
    _IMAGE_MAX_DIM = int(os.environ.get("IX_MCP_IMAGE_MAX_DIM", "1280"))
except ValueError:
    _IMAGE_MAX_DIM = 1280

# Max encoded size (bytes) of a single image returned to the model. The dimension
# cap alone does not bound bytes: a busy 1280px screenshot re-encoded as PNG can
# still be several megabytes, which floods the reply (and the model's context) and
# can exceed the host's per-image limit, so the host falls back to dumping the
# base64 as text. After the dimension cap an oversize image is therefore
# re-encoded as JPEG at descending quality -- and, if still over, downscaled
# further -- until it fits. Set ``IX_MCP_IMAGE_MAX_BYTES=0`` to disable the byte
# cap (the dimension cap still applies).
try:
    _IMAGE_MAX_BYTES = int(os.environ.get("IX_MCP_IMAGE_MAX_BYTES", "1000000"))
except ValueError:
    _IMAGE_MAX_BYTES = 1_000_000

# The custom mime the kernel hands the server to carry a job summary (mirrors
# outputs.JOB_MIME; duplicated so the kernel-side runtime stays import-light).
JOB_MIME = "application/x-ix-job+json"

# The custom mime a Result uses to carry the model-facing view (text plus
# images) for the server to unpack; it never reaches the dashboard render
# (it is not in _RICH_MIMES), so the human sees user_html and the model sees
# this. Mirrors outputs.IX_LLM_MIME.
IX_LLM_MIME = "application/x-ix-llm+json"

# The custom mime a Result uses to carry a STRUCTURED human view — a
# ``{"renderer": <name>, "data": <json>}`` spec the dashboard renders natively
# (pane_bridge republishes it as a `data` pane routed through the frontend's
# renderer registry) instead of a baked HTML string in a sandboxed frame.
# Mirrors outputs.IX_VIEW_MIME.
IX_VIEW_MIME = "application/x-ix-view+json"

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

# Cell semantics are Jupyter's: the last expression is the result, whatever its
# type, and stdout travels with it. `Result` survives as the OPT-IN way to split
# the human view from the model view (rich HTML vs concise text/images); a cell
# that never mentions it still returns exactly what a notebook would show.

# Opened lazily in install(); None when no store path is configured (the
# one-shot eval/exec paths, or a bare kernel started outside the server).
_store_conn = None
_store = None
_shell = None  # the InteractiveShell, set in install(); used to format rich results
_trace_file = None  # faulthandler dump target, kept open for the kernel's lifetime


def _rename_current_job(name: str) -> None:
    """Relabel the currently running job with ``name``.

    Sets ``job.name`` on the live :class:`Job` and, when a store connection is
    available, persists it so the dashboard reflects the new label immediately.
    Called by :func:`sh.sh` when the caller passes ``name=``. Best-effort:
    failures are silently swallowed so a store write never aborts user code.
    """
    job = _ix_current.get()
    if job is None:
        return
    job.name = name
    if _store is not None and _store_conn is not None:
        with contextlib.suppress(Exception):  # best-effort: a store write must not abort user code
            _store.rename(_store_conn, id=job.id, name=name)


class _Tee:
    """sys.stdout/err replacement that routes each write to the *current task's*
    job buffer (so concurrent jobs keep separate output) plus the real stream."""

    def __init__(self, original: Any) -> None:
        self._original = original

    def write(self, s: str) -> int:
        job = _ix_current.get()
        if job is not None:
            # Job output is captured here (and persisted to the store) rather than
            # streamed to IOPub, so the server reads it back from the job summary
            # and a backgrounded job's output is not lost to a stale cell context.
            job._append(s)
            return len(s)
        return self._original.write(s)

    def flush(self) -> None:
        with contextlib.suppress(Exception):  # flush failures on the wrapped kernel stream are non-fatal
            self._original.flush()

    def __getattr__(self, name: str) -> Any:
        return getattr(self._original, name)


class _CallableBool(int):
    """A bool that also answers ``()``: ``job.running`` and ``job.running()``
    both work. ``bool`` cannot be subclassed, so this is an int restricted to
    0/1; truthiness, comparison, and repr all behave like the bool it wraps."""

    def __call__(self) -> bool:
        return bool(self)

    def __repr__(self) -> str:
        return repr(bool(self))


class JobStillRunning(RuntimeError):
    """Raised by ``Job.result`` when the job has not finished yet.

    Reaching for a running job's result is the one job-polling footgun: a plain
    ``None`` reads as "finished with no value". This raises instead, so the
    confusion surfaces as a clear instruction to ``await`` it (or poll
    ``.done()``) rather than a silent wrong answer.
    """


class JobCancelled(RuntimeError):
    """Raised by ``await jobs['<id>']`` when the awaited job was cancelled.

    ``jobs`` is one kernel-wide registry, so several sessions may await the same
    job. A cancelled job used to throw ``CancelledError`` into every awaiting
    cell, which ``_runner`` then recorded as "cancelled" too -- one explicit
    ``jobs['<id>'].cancel()`` silently killed other agents' waiter cells (issue
    #2104). Raising an ordinary error instead keeps the awaiting cell alive and
    names the job that was cancelled.
    """


class Job:
    """A single ``python_exec`` execution: an awaitable handle over the asyncio
    task running the code, with its captured output, result, and status."""

    def __init__(
        self,
        code: str,
        name: str | None = None,
        budget: float = 15.0,
        kind: str = "cell",
        topic: str = "",
        session: str | None = None,
    ) -> None:
        self.id = uuid.uuid4().hex[:8]
        self.code = code
        self.name = name or self.id
        self.topic = topic
        # The MCP session id this run belongs to (None = the shared namespace).
        # Carried so a read helper running inside the cell can attribute its read
        # to the right session's redundant-read counters (see readstats).
        self.session = session
        # 'cell' for a normal execution; 'replay' for a re-run performed while
        # reopening a session file. Replays never feed future replays
        # (store.replayable filters on this), so a session cannot double-run
        # its history.
        self.kind = kind
        self.status = "running"
        self.started = time.time()
        # The foreground budget (seconds) this run was given before it backgrounds;
        # the dashboard draws a progress bar of elapsed-vs-budget while it runs.
        self.budget = float(budget)
        self.ended: float | None = None
        # The cell line currently executing, sampled off the suspended coroutine
        # chain by the flusher (see _current_line); None for a cell with no live
        # async frame. The dashboard highlights this line while the job runs.
        self.line: int | None = None
        # The cell line a failure was raised on (the deepest user frame of the
        # traceback, or a SyntaxError's reported line). None until/unless it fails.
        self.error_line: int | None = None
        # The cell's own coroutine / async generator, kept so _current_line can
        # read its suspended frame chain while the job runs.
        self._aobj = None
        # The cell's final value (a Result), exposed through the `result`
        # property; stored privately so an access while running can raise rather
        # than hand back a misleading None.
        self._result = None
        self.error: str | None = None
        # The actual exception a failed cell raised, kept so `await jobs['<id>']`
        # can re-raise it (with its original traceback) instead of handing back a
        # misleading None -- the documented "raises rather than return a
        # misleading None" contract. None while running, done, or cancelled.
        # `_exc_tb` pins the traceback as captured at failure time: each re-raise
        # restores it first, so awaiting the same failed job repeatedly does not
        # keep growing the shared exception object's traceback chain.
        self._exc: BaseException | None = None
        self._exc_tb: types.TracebackType | None = None
        self._buf: list[str] = []
        self._buflen = 0
        # Rich outputs (mime bundles) display()-ed while this job runs.
        self._displays: list[dict] = []
        self.task: asyncio.Task | None = None
        # Set by the SIGUSR2 wedge watchdog so _runner can tell its interrupt from
        # a KeyboardInterrupt the user's own code raised.
        self.interrupted_by_watchdog = False
        # The globals dict this job's code ran in (the shared user namespace, or
        # a per-session namespace -- see _session_ns). Set by __ix_run so the
        # bindings snapshot reads the namespace the cell actually wrote to.
        self._ns: dict | None = None
        # Set once __ix_run returns before the task finishes. When such a job
        # reaches a terminal state later, notify the agent session.
        self.backgrounded = False

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

    @property
    def text(self) -> str:
        """The finished run's model-facing result text (its `Result.llm_result`).

        The sibling of `.output` (stdout): `sh()`'s Output and a `Result` both
        answer `.text`, so a job handle does too, letting `jobs['id'].text` page
        the returned value without first reaching through `.result`. Empty while
        the job is still running (background it and `await jobs['id']` for the
        value); on a failed job it is empty too (the failure is in `.error`, and
        `await`-ing the job re-raises it)."""
        return _result_text(self)

    @property
    def pageable(self) -> str:
        """The text the paging helpers (`tail`/`head`/`slice`/`lines`/`grep`)
        operate on: this job's captured stdout, or -- when the cell printed
        nothing and its bulk is the returned value (a `Result`, or `sh()` output,
        whose text lives in the result, not stdout) -- the result's model-facing
        text. So the paging the over-cap notice advertises reaches a big returned
        value just as it reaches a big `print()`, never an empty buffer."""
        return self.output or _result_text(self)

    def tail(self, n: int = 2000) -> str:
        """Last ``n`` chars of this job's output (stdout, else its result text)."""
        return self.pageable[-n:]

    def head(self, n: int = 2000) -> str:
        """First ``n`` chars of this job's output (stdout, else its result text)."""
        return self.pageable[:n]

    def slice(self, start: int = 0, end: int | None = None) -> str:
        """A character window ``pageable[start:end]``, for paging a large output a
        chunk at a time after `grep`/`lines` locates the region."""
        return self.pageable[start:end]

    def lines(self, start: int = 0, end: int | None = None) -> str:
        """Output lines ``[start:end]`` (0-based, ``end`` exclusive), numbered to
        match `grep`'s line numbers so you can jump straight to a region."""
        numbered = self.pageable.splitlines()
        return "\n".join(f"{i}: {numbered[i]}" for i in range(*slice(start, end).indices(len(numbered))))

    def grep(
        self,
        pattern: str,
        ctx: int = 0,
        *,
        ignore_case: bool = True,
        max_matches: int = 200,
        max_chars: int = 20_000,
    ) -> str:
        """Lines of the captured output matching ``pattern`` (a regex), each with
        ``ctx`` lines of surrounding context and its line number. Capped at
        ``max_matches`` matches and ``max_chars`` total so the return is small
        enough to read in one reply; once you spot the region, widen with
        ``lines``/``slice``. Use this to find the needle in a truncated run
        instead of re-running the work."""
        import re as _re

        rx = _re.compile(pattern, _re.IGNORECASE if ignore_case else 0)
        src = self.pageable.splitlines()
        keep: list[int] = []
        seen: set[int] = set()
        matches = 0
        for index, line in enumerate(src):
            if rx.search(line):
                matches += 1
                if matches > max_matches:
                    break
                for j in range(max(0, index - ctx), min(len(src), index + ctx + 1)):
                    if j not in seen:
                        seen.add(j)
                        keep.append(j)
        keep.sort()
        rendered: list[str] = []
        prev: int | None = None
        for j in keep:
            if prev is not None and j > prev + 1:
                rendered.append("--")
            rendered.append(f"{j}: {src[j]}")
            prev = j
        body = "\n".join(rendered)
        if len(body) > max_chars:
            body = body[:max_chars] + f"\n... [grep output clipped to {max_chars} chars; use slice()]"
        if matches > max_matches:
            body += f"\n... [stopped at {max_matches} matches; narrow the pattern]"
        return body or f"(no lines match {pattern!r} in {len(src)} lines)"

    @property
    def running(self) -> _CallableBool:
        """True while the job runs. Works as an attribute (``job.running``) and
        as the historical method call (``job.running()``): both spellings are
        natural guesses, and the attribute form returning a bound method was a
        truthiness trap (every finished job looked "running" to ``getattr``)."""
        return _CallableBool(self.status == "running")

    @property
    def done(self) -> _CallableBool:
        """True once the job has finished (done, error, or cancelled), as an
        attribute or a call. Pair it with `.result`, which only yields a value
        once the job is done."""
        return _CallableBool(self.status != "running")

    @property
    def ok(self) -> bool:
        """True if the job finished successfully (no error, not cancelled)."""
        return self.status == "done"

    @property
    def result(self) -> Result | None:
        """This run's final value -- the `Result` the cell produced (or the one
        auto-wrapped from a bare displayable final expression like a DataFrame).

        Accessing it while the job is still running raises `JobStillRunning`,
        instead of returning a misleading `None`: background the work, then
        `await jobs['id']` to get the result, or poll `.done()` / `.running()`
        first. Once finished this is exactly what `await jobs['id']` yields.

        ``Job.result`` is a **property** (not a method), but since ``Result``
        is callable, both ``job.result`` and ``job.result()`` hand back the
        value. The returned Result's ``.text`` attribute is the rendered model
        text (same as ``.llm_result``), so ``job.result.text[-100:]`` pages it.
        Its ``.value`` is the ORIGINAL trailing-expression value (the DataFrame
        itself, not its rendered text), and subscripting delegates to it, so
        ``jobs['<id>'].result[0, 0]`` reads the real cell (issue #2068)."""
        if self.running():
            dur = time.time() - self.started
            raise JobStillRunning(
                f"job {self.id} is still running ({dur:.1f}s); "
                f"`await jobs['{self.id}']` to get its result, "
                f"or check `.done()` / `.running()` first"
            )
        return self._result

    def cancel(self) -> Job:
        if self.task is not None and not self.task.done():
            self.task.cancel()
        return self

    async def wait(self, timeout: float | None = None) -> Job:
        """Wait until this job finishes, or up to ``timeout`` seconds, and return
        the job (check ``.done()`` / ``.status`` / ``.result`` on it). Unlike
        ``await jobs['<id>']`` it never raises on a slow job -- it just returns
        the still-running handle at the deadline -- so one cell replaces a
        sleep-and-poll loop: ``(await jobs['ab12'].wait(30)).status``."""
        if self.task is not None and not self.task.done():
            await asyncio.wait({self.task}, timeout=timeout)
        return self

    def __await__(self) -> Any:
        # `await jobs['id']` should yield the job's result, but the runner task
        # returns None (it swallows the cell's exception to keep the shared kernel
        # alive), so wait for it then hand back the captured result -- or, if the
        # cell FAILED, re-raise the exception it raised. Returning `self._result`
        # unconditionally handed back None on a failed job, so `(await
        # jobs[id]).text` then died with an opaque AttributeError; the documented
        # contract is that awaiting raises rather than return a misleading None.
        async def _await_result() -> Result | None:
            if self.task is not None:
                try:
                    # Shield the job's task: ``jobs`` is one kernel-wide registry,
                    # so several sessions may await the same job, and an awaiter's
                    # own cancellation -- the common shape is an
                    # ``asyncio.wait_for(jobs['<id>'], t)`` timeout -- must not
                    # propagate INTO the shared task and kill the job for everyone
                    # (issue #2104). Cancelling a job stays explicit:
                    # ``jobs['<id>'].cancel()``.
                    await asyncio.shield(self.task)
                except asyncio.CancelledError:
                    current = asyncio.current_task()
                    if current is not None and current.cancelling():
                        # The AWAITER itself is being cancelled (its cell was
                        # cancelled, or its wait timed out): propagate that,
                        # leaving the job running for its other awaiters.
                        raise
                    if self.task.cancelled():
                        # The JOB was cancelled out from under this await. Surface
                        # an ordinary error naming the job, so the awaiting cell
                        # records what happened instead of dying "cancelled"
                        # itself (the cross-session cascade in issue #2104).
                        raise JobCancelled(
                            f"job {self.id} ({self.name}) was cancelled while "
                            f"awaited; jobs[{self.id!r}] holds its partial output"
                        ) from None
                    raise
            if self._exc is not None:
                # Re-raise the original exception object, so its type, message,
                # and traceback (the cell's own frames) all reach the caller --
                # the same failure they would have seen running the code inline.
                # Restore the traceback captured at failure time first, so a
                # repeated await re-raises from the same baseline instead of
                # accreting one more raise-frame chain per await.
                raise self._exc.with_traceback(self._exc_tb)
            return self._result

        return _await_result().__await__()

    def __repr__(self) -> str:
        dur = (self.ended or time.time()) - self.started
        at = f" L{line}" if self.running() and (line := _current_line(self)) else ""
        head = f"<Job {self.id} ({self.name}) [{self.status}{at}] {dur:.2f}s>"
        out = self.tail(800)
        return head + ("\n" + out if out else "")


def _nuon_key(value: Any) -> str:
    text = str(value)
    return text if re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", text) else json.dumps(text, ensure_ascii=False)


def _nuon_table(columns: list[Any], rows: list[Mapping[Any, Any]], *, _depth: int = 0) -> str:
    header = ", ".join(_nuon_key(c) for c in columns)
    if not rows:
        return f"[[{header}];]"
    body = ", ".join(
        "[" + ", ".join(_nuon(row.get(c), _depth=_depth + 1) for c in columns) + "]"
        for row in rows
    )
    return f"[[{header}]; {body}]"


def _nuon(value: Any, *, _depth: int = 0) -> str:
    """A compact Nushell NUON subset for model-facing structured output."""
    if _depth > 8:
        return json.dumps(_safe_repr(value), ensure_ascii=False)
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int) and not isinstance(value, bool):
        return str(value)
    if isinstance(value, float):
        if value == value and value not in (float("inf"), float("-inf")):
            return repr(value)
        return json.dumps(str(value), ensure_ascii=False)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=False)
    if isinstance(value, bytes):
        return json.dumps(base64.b64encode(value).decode("ascii"), ensure_ascii=False)
    if isinstance(value, Mapping):
        return "{" + ", ".join(f"{_nuon_key(k)}: {_nuon(v, _depth=_depth + 1)}" for k, v in value.items()) + "}"
    if isinstance(value, (list, tuple)):
        if value and all(isinstance(v, Mapping) for v in value):
            columns: list[Any] = []
            seen: set[str] = set()
            for row in value:
                for key in row:
                    text = str(key)
                    if text not in seen:
                        seen.add(text)
                        columns.append(key)
            return _nuon_table(columns, value, _depth=_depth)
        return "[" + ", ".join(_nuon(v, _depth=_depth + 1) for v in value) + "]"
    iso = getattr(value, "isoformat", None)
    if callable(iso):
        with contextlib.suppress(Exception):
            return json.dumps(iso(), ensure_ascii=False)
    return json.dumps(_safe_repr(value), ensure_ascii=False)


def _llm_text(value: Any) -> str | None:
    """Coerce a model-facing text field to ``str`` (or None to keep a default).

    ``llm_result`` flows into string paths (the job summary, ``Job.tail``
    paging, the reply text), so a non-str here -- most commonly a Result nested
    inside a Result, e.g. ``Result(llm_result=await browser.read(pg))`` -- used
    to surface later as an opaque ``TypeError`` deep in the runtime. Flatten a
    nested Result/Output to its own model text, and any other value to its repr,
    at construction time instead.
    """
    if value is None or isinstance(value, str):
        return value
    inner = getattr(value, "llm_result", None)
    if isinstance(inner, str):
        return inner
    return _nuon(value)


def _call_value_hook(value: Any, *names: str) -> Any:
    for name in names:
        hook = getattr(value, name, None)
        if hook is None:
            continue
        try:
            return hook() if callable(hook) else hook
        except Exception:
            return None
    return None


def _html_output(value: Any) -> str | None:
    html = _call_value_hook(value, "__ix_html__", "_ix_html_", "_repr_html_", "__html__")
    return html if isinstance(html, str) else None


def _llm_output(value: Any) -> str | None:
    out = _call_value_hook(value, "__ix_llm__", "_ix_llm_", "_repr_llm_")
    return _llm_text(out) if out is not None else None


def _result_from_text(cls: type, value: Any, *, html: str | None = None) -> Result:
    """The ``Result.text(...)`` constructor body (see :class:`_TextDescriptor`):
    a Result that shows the same text to the human and the model. Pass ``html``
    to give the human a richer view than the plain text."""
    body = value if isinstance(value, str) else _safe_repr(value)
    user = html if html is not None else f"<pre class=\"ix-result\">{_escape_html(body)}</pre>"
    return cls(user_html=user, llm_result=body)


class _TextDescriptor:
    """Let ``Result.text`` mean both things without colliding.

    On the class, ``Result.text("hi")`` is the constructor it has always been.
    On an instance, ``some_result.text`` is the already-rendered model text (the
    same string as ``.llm_result``), so reading a finished job's value works the
    way ``sh()``'s ``Output.text`` does: ``(await jobs['id']).text[-100:]``
    pages the text instead of dying with "'method' object is not subscriptable"
    (the bound constructor the old classmethod handed back)."""

    # Class access (`Result.text("hi")`) hands back the bound constructor; instance
    # access (`result.text`) hands back the rendered text. The overloads let the
    # checker see both, so `Result.text(...)` is callable and `result.text[-100:]`
    # is a str.
    @overload
    def __get__(self, obj: None, objtype: type | None = ...) -> Callable[..., Result]: ...
    @overload
    def __get__(self, obj: object, objtype: type | None = ...) -> str: ...
    def __get__(self, obj: object, objtype: type | None = None) -> Callable[..., Result] | str:
        if obj is None:
            return types.MethodType(_result_from_text, objtype)
        return obj.llm_result or ""


class Result:
    """Split a cell's final value into a human view and a model view.

    Entirely optional: a cell's last expression is its result, Jupyter-style,
    whatever its type, and the kernel renders it (rich types richly, plain
    values as their natural text) with the cell's stdout alongside. Reach for
    Result only when the two audiences should see DIFFERENT things. The
    dashboard renders ``user_html`` (a rich HTML view for the human watching);
    the model's tool result receives ``llm_result`` (concise text) plus any
    ``llm_images``. The two never cross: the human is not shown the model's
    text, and the model does not pay tokens for the HTML render.

    Construct it directly for full control, or use the shortcuts for the common
    cases::

        Result.text("done")                      # same text to human and model
        Result.ok("all 12 checks passed")        # a quiet confirmation
        Result.of(df)                            # render any value richly for
                                                 # the human, its repr to you
        Result(user_html="<b>hi</b>", llm_result="hi")
        Result(user_html=fig_html, llm_result="see plot", llm_images=[fig])

    ``llm_images`` items may be raw PNG/JPEG bytes, a base64 string, a data URI,
    a matplotlib Figure, a PIL image, or a path to an image file; each is sent to
    the model as a real image block, downscaled and re-encoded to fit the model
    image budget (``IX_MCP_IMAGE_MAX_DIM`` / ``IX_MCP_IMAGE_MAX_BYTES``), so a
    full-res screenshot never floods the reply. For an image meant only for the
    human, put it in ``user_html`` (an ``<img>`` data URI) and omit
    ``llm_images``: the dashboard shows the picture, the model pays no vision
    tokens at all. It is a mime bundle under the hood:
    ``text/html`` carries ``user_html`` and, when present, ``IX_LLM_MIME`` carries
    the model's text+images (unpacked by the server); ``text/plain`` carries the
    text as a fallback for plain hosts. ``user_view`` is the structured
    alternative to ``user_html``: a ``{"renderer": name, "data": ...}`` spec
    (carried as ``IX_VIEW_MIME``) the dashboard renders with a native component
    instead of a sandboxed HTML frame — prefer it when a registered renderer
    (e.g. ``file-view``) fits.

    On an instance, ``.text`` returns the already-rendered model text (same as
    ``.llm_result``), so ``(await jobs['id']).text[-100:]`` works. On the class,
    ``Result.text("hi")`` is the constructor it has always been. Calling a
    Result (``result()``) returns it unchanged, so ``jobs['id'].result()`` --
    the natural method-call guess at the ``Job.result`` property -- hands back
    the value instead of raising "'Result' object is not callable".
    """

    # Construct it however reads best. Pass the value(s) you want shown and it
    # does the right thing: `Result(x)` renders x richly for the human and hands
    # you its repr (exactly like `Result.of`), and `Result(a, b, ...)` shows each
    # value (so you never lose one to a silent positional). For full control give
    # the keywords, which always win: `Result(user_html=..., llm_result=...,
    # llm_images=[...])`.
    def __init__(self, *values: Any, user_html: str | None = None, user_view: dict | None = None, llm_result: str | None = None, llm_images: list | None = None) -> None:
        llm_result = _llm_text(llm_result)
        self.user_view = user_view
        # The original Python value this Result was built from (set by
        # `Result.of`, which also auto-wraps a cell's trailing expression), so
        # a finished job's REAL value -- the DataFrame itself, not its rendered
        # text -- stays reachable: `jobs['<id>'].result.value`, and
        # subscripting delegates to it (`jobs['<id>'].result[0, 0]`). None for
        # a Result built purely from keyword views (issue #2068).
        self.value: Any = None
        if user_html is not None or user_view is not None:
            self.user_html = user_html or ""
            self.llm_result = llm_result if llm_result is not None else ""
            self.llm_images = list(llm_images) if llm_images else []
            return
        if not values:
            # Result() / a text- or images-only Result built from keywords.
            self.user_html = ""
            self.llm_result = llm_result if llm_result is not None else ""
            self.llm_images = list(llm_images) if llm_images else []
            return
        built = (
            Result.of(values[0], llm_result=llm_result)
            if len(values) == 1
            else _result_from_values(values, llm_result=llm_result)
        )
        self.user_html = built.user_html
        self.user_view = built.user_view
        self.value = built.value
        self.llm_result = built.llm_result
        self.llm_images = list(llm_images) if llm_images else built.llm_images

    # ``Result.text("hi")`` constructs (the classic constructor); on an instance,
    # ``.text`` is the rendered model text (same as ``.llm_result``) -- see
    # _TextDescriptor. The two meanings used to collide: ``(await job).text``
    # returned the bound constructor, a guessing-game error when inspecting a
    # finished job.
    text = _TextDescriptor()

    @property
    def output(self) -> str:
        """The rendered model text, under the name ``Job.output`` and ``sh()``'s
        ``Output.output`` already answer to. ``await jobs['id']`` yields a
        Result while the un-awaited handle is a Job, so paging code holding
        "whichever one finished" reaches ``.output`` either way; before this
        alias that exact access died with an AttributeError, and inside a
        fire-and-forget watcher task it died silently (2026-07-02, 90 minutes
        of starved monitors -- the incident behind ``task_errors``)."""
        return self.llm_result or ""

    @classmethod
    def ok(cls, message: str = "done") -> Result:
        """A quiet confirmation for a side-effecting cell (an import, a cancel, a
        terminal keystroke) that has no value to return."""
        msg = str(message)
        user = f'<div class="ix-ok">\u2713 {_escape_html(msg)}</div>'
        return cls(user_html=user, llm_result=msg)

    @classmethod
    def of(cls, value: Any, *, llm_result: str | None = None) -> Result:
        """Wrap any value: render it richly for the human (a DataFrame as a
        table, a figure as an image, anything else as its display HTML or repr)
        and hand the model concise text. For a polars DataFrame the model text is
        compact NUON (the human still gets the styled HTML table), so a wide or
        long-stringed frame is never clipped to the agent the way the boxed text
        repr clips it. Override with ``llm_result`` or define ``__ix_llm__``.

        The built Result also keeps ``value`` itself reachable as ``.value``
        (and through subscripting), so the rendered wrapper never strands the
        original: ``jobs['<id>'].result[0, 0]`` reads the real DataFrame cell
        instead of dying inside the wrapper (issue #2068)."""
        built = cls._of(value, llm_result=llm_result)
        # A nested Result carries ITS original forward; anything else is the
        # original itself.
        built.value = value.value if isinstance(value, Result) else value
        return built

    @classmethod
    def _of(cls, value: Any, *, llm_result: str | None = None) -> Result:
        """The rendering body of :meth:`of` (which records ``.value`` on top)."""
        if isinstance(value, Result):
            # An existing Result is already split into its two views: copy it
            # faithfully (keeping llm_images) instead of rebuilding it from its
            # display bundle, which would drop the model image blocks. This also
            # preserves images when a nested Result is stacked below.
            return cls(
                user_html=value.user_html,
                user_view=value.user_view,
                llm_result=value.llm_result if llm_result is None else llm_result,
                llm_images=value.llm_images,
            )
        if not _is_polars_df(value):
            llm_hook = _llm_output(value)
            html_hook = _html_output(value)
            if html_hook is not None or llm_hook is not None:
                text_view = llm_result if llm_result is not None else (llm_hook or _nuon(value))
                user = html_hook if html_hook is not None else f'<pre class="ix-result">{_escape_html(text_view)}</pre>'
                return cls(user_html=user, llm_result=text_view)
        image_mime = _image_bytes_mime(value)
        if image_mime is not None:
            # Raw PNG/JPEG bytes (e.g. `await page.screenshot()`): show the human
            # the inline image and hand the model a real image block, not the
            # ~50k-char byte repr that would blow the result cap.
            img = _coerce_image(value)
            # _image_bytes_mime returned a mime, so value is real PNG/JPEG bytes
            # and _coerce_image always encodes them (never None on this path).
            assert img is not None
            user = f'<img alt="" src="data:{img["mime"]};base64,{img["data"]}" />'
            note = llm_result if llm_result is not None else f"[{image_mime} image, {len(bytes(value))} bytes]"
            return cls(user_html=user, llm_result=note, llm_images=[value])
        module = type(value).__module__ or ""
        if module.startswith(("matplotlib", "PIL")):
            # A figure or PIL image (e.g. `screen.capture()`): treat it exactly
            # like raw screenshot bytes -- inline image for the human, a real,
            # size-fitted image block for the model -- rather than leaving the
            # model a repr while a full-res PNG rides the display bundle (where
            # the byte cap would drop it entirely).
            img = _coerce_image(value)
            if img is not None:
                user = f'<img alt="" src="data:{img["mime"]};base64,{img["data"]}" />'
                note = llm_result if llm_result is not None else f"[{img['mime']} image]"
                return cls(user_html=user, llm_result=note, llm_images=[value])
        if isinstance(value, str):
            # A plain string is output, not a Python literal: hand the model the
            # string verbatim with terminal escapes stripped, so captured CLI /
            # ``--help`` / log text reads as itself instead of an escaped `repr`
            # full of ``\n`` and ``\x1b`` noise, and show the human the same text
            # with its ANSI color rendered to HTML. This is the read-tool
            # treatment for a streamed Result.
            text_view = llm_result if llm_result is not None else _strip_ansi(value)
            return cls(
                user_html=f'<pre class="ix-result">{_ansi_to_html(value)}</pre>',
                llm_result=text_view,
            )
        if _is_multi_rich(value):
            # A tuple/list that carries a rich element (a DataFrame, a figure, a
            # nested Result) is several things to SHOW, not one table: render each
            # element with its own view, stacked, rather than stringifying the rich
            # one into a `value` cell. `Result((repr_text, df))` thus shows the text
            # and the real table, not a 2-row frame of two reprs.
            return _result_from_values(list(value), llm_result=llm_result)
        frame = _frame_view(value)
        if frame is not None:
            # A rich result type (anything with ``_ix_to_frame_``) that exposes a
            # polars frame: render that frame the same as a bare DataFrame -- a
            # styled table for the human, compact NUON for the model -- so the
            # model reads the real rows, not a one-line summary repr.
            text_view = llm_result if llm_result is not None else _df_llm_text(frame)
            try:
                import view as _view

                return cls(user_html=_view.df_html(frame), llm_result=text_view)
            except Exception:
                user = f'<pre class="ix-result">{_escape_html(text_view)}</pre>'
                return cls(user_html=user, llm_result=text_view)
        value = _as_frame_if_tabular(value)
        if llm_result is not None:
            text_view = llm_result
        elif _is_polars_df(value):
            text_view = _df_llm_text(value)
        else:
            text_view = _nuon(value)
        if _is_polars_df(value):
            # A frame (incl. a dict/records value coerced above) renders as the
            # dashboard's styled table directly -- a table for the human, compact
            # NUON for you -- and works even without the IPython display formatter.
            try:
                import view as _view

                return cls(user_html=_view.df_html(value), llm_result=text_view)
            except Exception:
                user = f'<pre class="ix-result">{_escape_html(text_view)}</pre>'
                return cls(user_html=user, llm_result=text_view)
        bundle = _result_bundle(value)
        data = (bundle or {}).get("data", {})
        # Preserve a structured view riding the value's display bundle (e.g. a
        # view.Code as the cell's trailing expression), so wrapping in Result
        # never downgrades the dashboard render to the HTML fallback. The
        # normalized bundle JSON-encodes custom mimes.
        view_spec = data.get(IX_VIEW_MIME)
        if isinstance(view_spec, str):
            try:
                view_spec = json.loads(view_spec)
            except json.JSONDecodeError:
                view_spec = None
        if not isinstance(view_spec, dict):
            view_spec = None
        if "text/html" in data:
            user = data["text/html"]
        elif "image/png" in data:
            user = f'<img alt="" src="data:image/png;base64,{data["image/png"]}" />'
        elif "image/svg+xml" in data:
            user = data["image/svg+xml"]
        else:
            user = f'<pre class="ix-result">{_escape_html(text_view)}</pre>'
        return cls(user_html=user, user_view=view_spec, llm_result=text_view)

    def _repr_mimebundle_(self, **_kwargs: Any) -> dict:
        # IPython's display protocol: html is the human view (the dashboard
        # prefers it); IX_VIEW_MIME carries a structured human view the
        # dashboard renders natively (preferred over the html when present);
        # IX_LLM_MIME carries the model's text+images, which the server unpacks
        # and the dashboard ignores; text/plain is the fallback. An EMPTY html
        # view is omitted, not advertised — a host that ranks text/html above
        # text/plain would otherwise render a blank result.
        bundle: dict = {"text/plain": self.llm_result or ""}
        if self.user_html:
            bundle["text/html"] = self.user_html
        if self.user_view is not None:
            bundle[IX_VIEW_MIME] = self.user_view
        images = [img for img in (_coerce_image(i) for i in self.llm_images) if img]
        bundle[IX_LLM_MIME] = {"text": self.llm_result or "", "images": images}
        return bundle

    @property
    def output(self) -> str:
        """Alias for the model text (same as ``.text`` / ``.llm_result``).

        A live/errored job handle exposes ``.output`` (its captured stdout) and
        ``sh()``'s Output exposes ``.output`` too; a finished ``Result`` is the
        third thing an agent pages, so it answers the same attribute -- reading a
        result via ``.output`` returns its model text instead of dying with an
        AttributeError and a guessing game about which surface owns which name."""
        return self.llm_result or ""

    def __call__(self) -> Result:
        """Calling a Result returns it unchanged. ``Job.result`` is a property,
        so ``jobs['id'].result()`` -- the natural method-call guess while
        polling a finished job -- used to die with "'Result' object is not
        callable". Property and call now both hand over the value."""
        return self

    def __getitem__(self, key: Any) -> Any:
        """Subscript through to the wrapped original value: ``jobs['<id>']
        .result[0, 0]`` -- the natural way to reach a finished cell's DataFrame
        cell -- reads the real value instead of dying on the rendered-text
        wrapper (issue #2068). A Result that carries no original (built purely
        from keyword views) raises a TypeError that points at ``.text``."""
        if self.value is not None:
            return self.value[key]
        raise TypeError(
            "this Result wraps no subscriptable value; read `.text` for its rendered text"
        )

    def __repr__(self) -> str:
        # Plain-text fallback (the stored result repr, non-rich hosts): the model
        # view, never the HTML.
        return self.llm_result or ""


def _as_frame_if_tabular(value: Any) -> Any:
    """A mapping (a config dict, counts) or a list of mappings (records) is
    tabular: render it as a polars frame -- a styled table for the human, compact
    NUON for you -- rather than a raw dict/list repr. Anything else is returned
    unchanged. Keeps `Result({...})` from shoving a dict under text/html (invalid)
    and from collapsing to a bare repr."""
    try:
        import polars as pl
    except Exception:
        return value
    if isinstance(value, Mapping) and value:
        vals = list(value.values())
        # A dict whose values are themselves mappings is NESTED data: render the
        # value column as a polars Struct so `view.df_html` shows each value as a
        # recursive nushell-style sub-table rather than a clipped repr. Scalar or
        # heterogeneous values fall back to the flat key/value repr below.
        if all(isinstance(v, Mapping) for v in vals):
            with contextlib.suppress(Exception):  # best-effort: fall back to flat repr on any polars error
                return pl.DataFrame(
                    {"key": [str(k) for k in value], "value": pl.Series([dict(v) for v in vals])}
                )
        return pl.DataFrame(
            {"key": [str(k) for k in value], "value": [_safe_repr(v) for v in value.values()]}
        )
    if isinstance(value, (list, tuple)) and value and all(isinstance(x, Mapping) for x in value):
        try:
            return pl.DataFrame(list(value))
        except Exception:
            return value
    if isinstance(value, (list, tuple)) and value:
        # A plain list/tuple of scalars is tabular too: one styled `value` column
        # for the human, compact NUON for you. (Lists of mappings are records above.)
        try:
            return pl.DataFrame({"value": list(value)})
        except Exception:
            try:
                return pl.DataFrame({"value": [_safe_repr(v) for v in value]})
            except Exception:
                return value
    return value


def _is_rich_element(value: Any) -> bool:
    """True if ``value`` carries its own rich view (a DataFrame, a figure/image,
    an htpy element, or a Result), so flattening it into a one-column frame would
    throw that view away. Plain scalars and containers are not rich."""
    return isinstance(value, Result) or _is_polars_df(value) or _is_displayable(value)


def _is_multi_rich(value: Any) -> bool:
    """True for a non-empty list/tuple that carries at least one rich element, so
    ``Result.of`` should stack each element's view instead of coercing the whole
    sequence to a single table. A list/tuple of plain scalars (or of mappings)
    stays tabular -- only a sequence mixing in a DataFrame/figure/Result needs the
    stacked treatment."""
    return isinstance(value, (list, tuple)) and bool(value) and any(_is_rich_element(v) for v in value)


def _result_from_values(values: Any, *, llm_result: str | None = None) -> Result:
    """Render several values as one Result: each value's rich view stacked for the
    human (so `Result(a, b)` shows BOTH, never just the first), their reprs joined
    for you. `llm_result` overrides the joined model text."""
    items = [Result.of(v) for v in values]
    user_html = "".join(
        f'<div class="ix-result-item" data-ix-index="{i}">{item.user_html}</div>'
        for i, item in enumerate(items)
    )
    text = llm_result if llm_result is not None else chr(10).join(item.llm_result for item in items)
    images: list = []
    for item in items:
        images.extend(item.llm_images)
    return Result(user_html=user_html, llm_result=text, llm_images=images)


class Resource:
    """A live, self-updating HTML view that lives as long as its source does.

    Where a :class:`Result` is a cell's *final* value rendered once, a Resource
    is a living thing the kernel keeps re-rendering: a running terminal, a custom
    widget, anything with a current HTML representation. Register one with
    :func:`register_resource`; while it stays alive the runtime mirrors its
    latest HTML to the store every flush tick and the dashboard sidebar shows
    all resources updating in place. The resource closes itself (switches to a
    closed indicator while keeping its final pane) when its ``alive`` predicate
    reports the source is gone.

    Pass ``actions={"name": handler}`` to make the resource interactive: its HTML
    gets ``ix.act(name, payload)`` and ``ix.events(fn)`` injected (see
    :attr:`script`), each ``act`` runs the named in-kernel handler, and every
    handler result/error -- plus any agent ``reply`` -- streams back to the page
    over the resource's live event feed.
    """

    def __init__(
        self,
        id: str,
        title: str,
        kind: str,
        render: Any,
        alive: Any = None,
        actions: Mapping[str, Any] | None = None,
        execution_id: str = "",
    ) -> None:
        self.id = id
        self.title = title
        self.kind = kind
        self.execution_id = execution_id
        self._render = render
        self._alive = alive
        self.status = "live"
        self.created = time.time()
        self.html = ""
        self.error: str | None = None
        self.actions: dict[str, Any] = dict(actions) if actions else {}
        self._action_channel: Input | None = None
        self._dispatcher: asyncio.Task | None = None
        if self.actions:
            self._start_actions()

    def closed(self) -> bool:
        return self.status == "closed"

    def close(self) -> Resource:
        """Close the resource and tear down any action channel/dispatcher."""
        self.status = "closed"
        if self._dispatcher is not None:
            self._dispatcher.cancel()
            self._dispatcher = None
        if self._action_channel is not None:
            self._action_channel.close()
            self._action_channel = None
        return self

    def alive(self) -> bool:
        if self.closed():
            return False
        if self._alive is None:
            return True
        try:
            return bool(self._alive())
        except Exception:
            # A source whose liveness check raises is treated as gone.
            return False

    async def _render_out(self) -> Any:
        """Invoke the render, awaiting it if it is a coroutine. The raw output,
        before any HTML coercion: an html resource stringifies it, a `data`
        resource keeps the structure (a ``{"renderer", "data"}`` spec)."""
        out = self._render() if callable(self._render) else self._render
        if inspect.iscoroutine(out):
            out = await out
        return out

    async def render_html(self) -> str:
        """The current HTML for this resource (awaits the render if it is async).
        An interactive resource gets its wiring script prepended, so the page's
        markup can call ``ix.act``/``ix.events`` without including anything."""
        out = await self._render_out()
        html = out if isinstance(out, str) else str(out)
        script = self.script
        return script + html if script else html

    async def render_view(self) -> dict:
        """The current structured view for a ``kind="data"`` resource: a
        ``{"renderer": name, "data": <json>}`` spec the dashboard renders with a
        native component (via the frontend renderer registry) instead of a
        sandboxed HTML frame. The counterpart to :meth:`render_html`, for a live,
        self-updating pane that wants a real renderer rather than baked markup
        (e.g. the `nix` module's live build tree). The render must return that
        dict; anything else is an error the sweep surfaces on the pane."""
        out = await self._render_out()
        if not (isinstance(out, dict) and isinstance(out.get("renderer"), str) and "data" in out):
            raise TypeError(
                "a data resource's render must return {'renderer': str, 'data': ...}; "
                f"got {type(out).__name__}"
            )
        return out

    @property
    def script(self) -> str:
        """The ``<script>`` an interactive resource's HTML is served with ("" for
        a plain resource). It extends the shared ``window.ix`` object with:

        - ``ix.act(name, payload) -> Promise``: queue ``payload`` for the named
          in-kernel action handler (rides the existing ``/api/input`` write path,
          so it shares that endpoint's network-boundary authorization). Resolves
          with ``{ok, call}``; the handler's return value arrives on the event
          feed as ``{kind: 'action_result', call, value}``.
        - ``ix.events(fn) -> EventSource``: subscribe to this resource's live
          feed (action results, errors, and agent ``reply`` messages), invoking
          ``fn(event)`` per event.
        """
        channel = self._action_channel
        if channel is None:
            return ""
        base = os.environ.get("IX_MCP_DATA_API_URL", "").rstrip("/")
        events_url = f"{base}/api/resources/{self.id}/events"
        # These interpolate into a <script> body. channel.endpoint is env-derived
        # and channel.id is a secrets token; self.id is validated to [A-Za-z0-9._-]
        # at register_resource (an interactive resource), so none can carry a
        # `</script>` breakout. json.dumps still quotes them as JS string literals.
        return (
            "<script>(function(){"
            f"var E={json.dumps(channel.endpoint)},C={json.dumps(channel.id)},"
            f"S={json.dumps(events_url)};"
            "var x=(window.ix=window.ix||{});"
            "x.act=function(a,p){var id=Math.random().toString(36).slice(2,10);"
            "return fetch(E,{method:'POST',"
            "headers:{'Content-Type':'text/plain;charset=UTF-8'},"
            "body:JSON.stringify({channel:C,payload:{action:a,call:id,"
            "payload:p===undefined?null:p}})})"
            ".then(function(r){return r.json();})"
            ".then(function(j){j.call=id;return j;});};"
            "x.events=function(f){var s=new EventSource(S);"
            "s.onmessage=function(e){try{f(JSON.parse(e.data));}catch(_){}};"
            "return s;};"
            "})();</script>"
        )

    def _start_actions(self) -> None:
        """Open the action channel and start the dispatcher consuming it."""
        self._action_channel = Input(title=f"{self.title} actions")
        with contextlib.suppress(RuntimeError):  # no loop (sync test context): actions need the kernel loop
            self._dispatcher = asyncio.get_event_loop().create_task(self._dispatch_actions())

    def _emit_event(self, kind: str, body: dict) -> None:
        """Append one event to this resource's live feed (best-effort: the page
        stream is a convenience; a store failure must not abort the handler)."""
        if _store is None or _store_conn is None:
            return
        with contextlib.suppress(Exception):  # best-effort: a store write must not abort user code
            _store.add_event(
                _store_conn,
                resource=self.id,
                kind=kind,
                body=json.dumps(body, default=_safe_repr),
            )

    async def _dispatch_actions(self) -> None:
        """Consume the action channel: run each queued ``ix.act`` submission's
        handler and stream its result (or error) back on the event feed."""
        channel = self._action_channel
        if channel is None:
            return
        async for submission in channel:
            name = submission.get("action") if isinstance(submission, dict) else None
            call = submission.get("call") if isinstance(submission, dict) else None
            handler = self.actions.get(name)
            if handler is None:
                self._emit_event("error", {"action": name, "call": call, "error": f"no such action {name!r}"})
                continue
            try:
                out = handler(submission.get("payload") if isinstance(submission, dict) else None)
                if inspect.isawaitable(out):
                    out = await out
            except Exception as exc:
                self._emit_event(
                    "error",
                    {
                        "action": name,
                        "call": call,
                        "error": "".join(traceback.format_exception_only(type(exc), exc)).strip(),
                    },
                )
                continue
            self._emit_event("action_result", {"action": name, "call": call, "value": out})

    def __repr__(self) -> str:
        return f"<Resource {self.id} ({self.title}) [{self.status}] {self.kind}>"


def register_resource(
    source: Any = None,
    *,
    title: str | None = None,
    render: Any = None,
    id: str | None = None,
    kind: str = "html",
    alive: Any = None,
    actions: Mapping[str, Any] | None = None,
) -> Resource:
    """Register a live HTML resource: a view the dashboard shows in its sidebar.

    A resource is a *live* HTML pane (unlike ``cells``, which are static
    snapshots): its ``render`` is re-invoked on refresh, so returning fresh HTML
    updates the pane in place. Use it for a dashboard you want to keep glancing at
    -- a status board, a queue, a metric -- rather than a one-shot result.

    Arguments::

        register_resource(render=lambda: html, title="queue", id="queue")  # callable
        register_resource(obj)            # obj.resource_html() / obj.to_html()
        register_resource(render=fn, alive=lambda: job.running)  # auto-closes

    - ``render``: a callable (sync or async) returning the current HTML. Or pass a
      ``source`` object with ``resource_html()`` / ``to_html()``.
    - ``id``: give a STABLE id so re-registering REPLACES the same resource (a loop
      updating one view), instead of spawning a new pane each call. Omitted -> a
      random id, i.e. a new resource every time.
    - ``alive``: optional predicate; when it returns False the resource closes
      itself but keeps its final pane. Else call ``.close()`` on the returned handle.

    Viewing it as a native overlay window (macOS): besides the web dashboard, the
    ``ix-windows`` consumer renders each live resource as its own floating, blurred,
    auto-sizing overlay window. Run it alongside your session::

        nix run .#ix-windows

    Windows open/close/refresh automatically as resources appear, update, and
    close. Move one by dragging its card chrome (the padding around the content);
    it is not resizable (size follows the content).

    STYLING: write content, not a page. The host (dashboard pane and overlay
    window alike) already provides the card: a translucent, rounded surface that
    on macOS blurs whatever is behind the window, plus the close control and the
    system font (SF via ``-apple-system``, antialiased; ``code``/``pre`` are
    monospace). For a native look:

    - Do NOT set a background on ``html``/``body`` or paint a full-bleed
      wrapper: an opaque background covers the blur and the card reads as a flat
      rectangle. Leave the page transparent; tint only small elements
      (badges, rows) and prefer translucent tints (``rgba``) over solid fills.
    - Do NOT add your own card chrome: no page-level ``border-radius``,
      ``box-shadow``, or outer border; the host draws those.
    - Add your own padding (the host renders content edge to edge), size
      intrinsically (the window auto-fits the content; avoid ``100vw``/fixed
      page widths), and inherit the host font instead of restating one.

    Interactive (buttons/forms that run python): pass ``actions`` -- a dict of
    name -> handler (sync or async, called with the submitted payload). The HTML
    is served with ``ix.act(name, payload)`` and ``ix.events(fn)`` pre-wired.
    For any non-trivial UI prefer ``svelte.component(...)`` (module ``svelte``)
    over hand-written HTML/JS strings: it compiles a real Svelte 5 component to
    a self-contained bundle over this same wiring, with one reactive renderer
    instead of a server template plus a hand-rolled ``ix.events`` redraw. ::

        async def on_deploy(payload):
            run = await start_deploy(payload["env"])
            await notify(f"deploy requested: {run}", resource="deploy-panel")
            return {"run": run}

        register_resource(
            render=lambda: '<button onclick="ix.act(\\'deploy\\', {env: \\'prod\\'})">ship</button>',
            id="deploy-panel", actions={"deploy": on_deploy},
        )

    Each ``ix.act`` queues its payload for the handler; the handler's return value
    (or error) streams back to the page as ``{kind: 'action_result', call, value}``
    on the feed ``ix.events(fn)`` subscribes to, alongside any agent ``reply``.
    Call :func:`notify` (with ``resource=<id>``) in every handler by default, as
    above: it is the only way the agent session learns the human acted (the
    page<->kernel loop runs without the agent, so an unwired handler means the
    agent must poll kernel state). Skip it only for purely page-local
    interactions. For a
    simple one-question form, :func:`ask` is still the shortcut; the lower-level
    :class:`Input` + ``.script`` path also still works. A cross-origin ``fetch``
    to the data API DOES work from the sandbox (that is how input flows back);
    only *same-origin* access to the embedding page is blocked.

    Prefer SELF-CONTAINED HTML. The body is rendered inside a sandboxed,
    opaque-origin ``<iframe>`` (``sandbox="allow-scripts"``, no
    ``allow-same-origin``) in both the dashboard and the overlay, so same-origin
    ``fetch``, cookies, and storage are unavailable. Absolute HTTPS scripts/styles
    may still load (subject to the browser and the remote server / CORS), but they
    are a live network dependency, not an isolated pane -- and an ES-module
    ``import`` from a CDN was observed to fail under the opaque origin. For
    reproducible, offline panes, pre-render anything needing a library and embed
    the static output -- e.g. render a mermaid diagram to SVG server-side
    (``kroki.io``, the ``mermaid`` CLI, ...) and put the static ``<svg>`` in the
    HTML.

    Returns the :class:`Resource` handle (call ``.close()`` to remove it).
    """
    if render is None:
        if source is None:
            raise ValueError("register_resource needs a render callable or a source object")
        if hasattr(source, "resource_html"):
            render = source.resource_html
        elif hasattr(source, "to_html"):
            render = source.to_html
        elif callable(source):
            render = source
        else:
            raise TypeError(
                f"{type(source).__name__} has no resource_html()/to_html(); pass render="
            )
    if title is None:
        title = getattr(source, "title", None) or (
            type(source).__name__ if source is not None else "resource"
        )
    rid = id or uuid.uuid4().hex[:8]
    # An interactive resource's id is interpolated into the injected <script> and
    # into its `/api/resources/{id}/events` SSE route, so it must be a safe token:
    # `json.dumps` does NOT escape `</script>` or `/`, so an id carrying `</script>`
    # would break out of the script tag (XSS in the pane), and a `/` would silently
    # miss the SSE route. Restrict it to url/path-safe characters -- random ids
    # (uuid hex) and Input tokens (`secrets.token_urlsafe`) already satisfy this;
    # an agent-supplied id built from external data (a repo/branch/chat id) is the
    # one that could smuggle markup. Non-interactive resources never reach either
    # sink, so they keep accepting any id.
    if actions and not _RESOURCE_ID_RE.fullmatch(rid):
        raise ValueError(
            f"an interactive resource id must match [A-Za-z0-9._-]+ (it is embedded "
            f"in a <script> and a URL path); got {rid!r}"
        )
    # Re-registering an id REPLACES that resource; close the old one first so its
    # action channel/dispatcher never outlives the pane it served.
    old = resources.get(rid)
    if old is not None:
        old.close()
    current = _ix_current.get()
    execution_id = getattr(current, "id", "") if current is not None else ""
    res = Resource(rid, str(title), kind, render, alive, actions=actions, execution_id=execution_id)
    resources[rid] = res
    return res


class Jobs(dict[str, Job]):
    """The kernel-wide run registry (``jobs``): every execution this kernel has
    run, keyed by job id. Beyond the dict surface, :meth:`spawn` registers an
    ad-hoc awaitable as a first-class background job, so work an agent started
    itself (a coroutine, a Task) gets the same lifecycle as a backgrounded cell:
    a dashboard card, a completion notification, and a pageable/awaitable
    ``jobs['<id>']`` handle (issue #2164)."""

    def spawn(self, aw: Awaitable[Any], *, name: str | None = None, topic: str | None = None) -> Job:
        """Register ``aw`` (any awaitable: a coroutine, Task, or Future) as a
        first-class background job and return its :class:`Job` handle at once.

        The awaitable gets the full job lifecycle: it appears in ``jobs`` /
        ``history()`` and on the dashboard, its completion pushes a channel
        notification exactly like a backgrounded cell, and its value is
        retrieved with ``await jobs['<id>']`` (or ``.result`` once done; a
        failure re-raises there, like any other job). ``name`` labels the job
        (defaults to the coroutine's qualname); ``topic`` files it under a
        dashboard topic (defaults to the session's current one). Must be called
        with the kernel's event loop running (i.e. from inside a cell)."""
        if not inspect.isawaitable(aw):
            raise TypeError(
                f"jobs.spawn() needs an awaitable (coroutine, Task, or Future), got {type(aw).__name__}"
            )
        # Raises RuntimeError outside the loop, BEFORE the job is registered, so
        # a misuse never leaves a forever-"running" phantom in the registry.
        loop = asyncio.get_running_loop()
        parent = _ix_current.get()
        label = name or getattr(aw, "__qualname__", None) or type(aw).__name__
        job = Job(
            f"jobs.spawn({label!r})",
            name=label,
            # No foreground wait: a spawned job is background from birth, so
            # there is no budget window to draw (0 is /api/exec's floor too).
            budget=0.0,
            kind="spawn",
            topic=topic or session.topic,
            session=parent.session if parent is not None else None,
        )
        # Background from birth: completion notifies the agent session the same
        # way a budget-expired cell does (see the runners' shared finally tail).
        job.backgrounded = True
        self[job.id] = job
        job.task = loop.create_task(_spawn_runner(job, aw))
        return job


jobs: Jobs = Jobs()
resources: dict[str, Resource] = {}

# A safe id for an INTERACTIVE resource: it is interpolated into the injected
# <script> body and into the `/api/resources/{id}/events` URL path, so it must
# carry no HTML/JS or path metacharacters (see register_resource).
_RESOURCE_ID_RE = re.compile(r"[A-Za-z0-9._-]+")


# --------------------------------------------------------------------------- #
# Interactive input: the browser -> kernel path that lets a rendered resource
# (or cell) carry a form the human fills in, so the agent can pop a window up and
# await the reply. An `Input`'s id is an ADDRESS, not a secret: it rides in the
# rendered HTML, which the read endpoints and the dashboard hub serve to anyone on
# the bind, so `/api/input` authorizes by the network boundary (loopback or a
# trusted tailnet), the same posture as `/api/exec`. The injected `ixSubmit(payload)`
# posts to that endpoint; the server appends to the store's `inputs` queue; the kernel's flush
# tick (`_drain_inputs`) delivers each payload here and the agent's `await`
# resolves. No same-origin access, cookies, or hub plumbing involved: an
# opaque-origin sandboxed iframe can still make a cross-origin `fetch`.
# --------------------------------------------------------------------------- #

input_channels: dict[str, Input] = {}


def _escape_attr(text: str) -> str:
    """Escape a string for an HTML double-quoted attribute value. ``_escape_html``
    covers text nodes (``&<>``); an attribute also needs the quote escaped so a
    value containing ``"`` cannot break out of the attribute."""
    return _escape_html(str(text)).replace('"', "&quot;")


class Input:
    """A live input channel: render arbitrary HTML with a form and await the reply.

    Where a :class:`Resource` is output the kernel pushes to the human, an
    ``Input`` is the channel the human pushes back on. Create one, drop its
    :attr:`script` into any HTML you render (a resource, a cell), and have your
    markup call the injected ``ixSubmit(payload)`` -- each call delivers
    ``payload`` here. Read submissions with ``await channel`` (the next one) or
    ``async for payload in channel`` (a stream):

        inp = Input(title="name")
        register_resource(
            render=lambda: inp.script + "<button onclick='ixSubmit({clicked:1})'>go</button>",
            id=inp.id, title="name",
        )
        payload = await inp            # -> {"clicked": 1}

    For the common "ask a question, get an answer" case use :func:`ask`, which
    builds the form, pops the window, and returns the response for you.

    ``await`` blocks until the human responds, which will exceed your
    ``python_exec`` budget and background the run; read the answer later with
    ``await jobs['<id>']``.
    """

    def __init__(self, title: str = "input", id: str | None = None) -> None:
        # The id addresses this channel in submissions; it is not an auth secret
        # (it rides in the served HTML). Mint a long random token anyway so ids do
        # not collide and are not enumerable, but `/api/input` gates on the network
        # boundary, not on knowing this id.
        self.id = id or secrets.token_urlsafe(18)
        self.title = title
        self.status = "open"
        self.created = time.time()
        self._queue: asyncio.Queue = asyncio.Queue()
        input_channels[self.id] = self
        # Open the channel in the store immediately so a submission that races the
        # first render (the human clicks fast) is still authorized, not 404'd.
        if _store is not None and _store_conn is not None:
            with contextlib.suppress(Exception):  # best-effort: a missing store just means no submissions arrive
                _store.open_channel(_store_conn, id=self.id, title=self.title)

    @property
    def endpoint(self) -> str:
        """The absolute ``/api/input`` URL the injected script posts to. Absolute
        because the sandboxed iframe's origin is ``about:srcdoc`` -- a relative URL
        would resolve against that, not the data API."""
        base = os.environ.get("IX_MCP_DATA_API_URL", "").rstrip("/")
        return f"{base}/api/input"

    @property
    def script(self) -> str:
        """A ``<script>`` to embed in the resource HTML. It defines
        ``window.ixSubmit(payload)`` (POST ``payload`` to this channel) and the
        lower-level ``window.ix.submit(channelId, payload)`` for HTML driving more
        than one channel. Posts ``text/plain`` so the cross-origin request stays
        a CORS *simple* request (no preflight); the server parses the JSON body
        regardless of content type."""
        # endpoint/id are a URL and a urlsafe token: both safe inside a JS string.
        return (
            "<script>(function(){"
            f'var E={json.dumps(self.endpoint)},C={json.dumps(self.id)};'
            "var x=(window.ix=window.ix||{});"
            "x.submit=function(c,p){return fetch(E,{method:'POST',"
            "headers:{'Content-Type':'text/plain;charset=UTF-8'},"
            "body:JSON.stringify({channel:c,payload:p})}).then(function(r){return r.json();});};"
            "window.ixSubmit=function(p){return x.submit(C,p);};"
            "})();</script>"
        )

    def _deliver(self, payload: Any) -> None:
        """Hand a drained submission to the awaiting coroutine (kernel-internal)."""
        self._queue.put_nowait(payload)

    async def recv(self, timeout: float | None = None) -> Any:
        """The next submission's payload. Waits forever by default; pass
        ``timeout`` (seconds) to raise ``TimeoutError`` instead."""
        if timeout is None:
            return await self._queue.get()
        return await asyncio.wait_for(self._queue.get(), timeout)

    def __await__(self) -> Any:
        return self.recv().__await__()

    def __aiter__(self) -> Input:
        return self

    async def __anext__(self) -> Any:
        return await self._queue.get()

    def closed(self) -> bool:
        return self.status == "closed"

    def close(self) -> Input:
        """Close the channel: `/api/input` stops accepting submissions for it and
        its queued-but-undelivered inputs are dropped. Idempotent."""
        self.status = "closed"
        input_channels.pop(self.id, None)
        if _store is not None and _store_conn is not None:
            with contextlib.suppress(Exception):  # best-effort: closing is advisory once the awaiter is gone
                _store.close_channel(_store_conn, id=self.id)
        return self

    def __repr__(self) -> str:
        return f"<Input {self.id} ({self.title}) [{self.status}]>"


def _ask_form_html(
    channel: Input,
    prompt: str,
    *,
    fields: list[str] | None,
    choices: list[str] | None,
    multiline: bool,
    submit_label: str,
) -> str:
    """The self-contained form HTML :func:`ask` renders: a prompt, the inputs, and
    a wiring script that gathers the values and calls ``ixSubmit``. Arranged for the
    dashboard's light/dark surface; every agent-supplied string is escaped."""
    if choices is not None:
        body = "".join(
            f'<button type="button" class="ix-choice" data-ix-value="{_escape_attr(c)}">'
            f"{_escape_html(str(c))}</button>"
            for c in choices
        )
        controls = f'<div class="ix-choices">{body}</div>'
    elif fields is not None:
        controls = "".join(
            f'<label class="ix-field"><span>{_escape_html(str(f))}</span>'
            f'<input name="{_escape_attr(f)}" autocomplete="off"></label>'
            for f in fields
        ) + f'<button type="submit" class="ix-submit">{_escape_html(submit_label)}</button>'
    else:
        control = (
            '<textarea name="value" rows="3" autofocus></textarea>'
            if multiline
            else '<input name="value" autocomplete="off" autofocus>'
        )
        controls = f'{control}<button type="submit" class="ix-submit">{_escape_html(submit_label)}</button>'
    style = (
        "<style>"
        ".ix-ask{font:14px/1.5 ui-sans-serif,system-ui,sans-serif;padding:14px 16px;"
        "max-width:34rem}"
        ".ix-prompt{font-weight:600;margin-bottom:10px}"
        ".ix-field{display:flex;flex-direction:column;gap:3px;margin-bottom:8px}"
        ".ix-field span{font-size:12px;opacity:.7}"
        ".ix-ask input,.ix-ask textarea{font:inherit;padding:6px 8px;border:1px solid "
        "color-mix(in srgb,currentColor 25%,transparent);border-radius:6px;background:transparent;"
        "color:inherit;width:100%;box-sizing:border-box}"
        ".ix-choices{display:flex;flex-wrap:wrap;gap:8px}"
        ".ix-ask button{font:inherit;cursor:pointer;padding:6px 12px;border-radius:6px;"
        "border:1px solid color-mix(in srgb,currentColor 30%,transparent);background:"
        "color-mix(in srgb,currentColor 8%,transparent);color:inherit}"
        ".ix-ask button:hover{background:color-mix(in srgb,currentColor 16%,transparent)}"
        ".ix-submit{margin-top:10px}"
        ".ix-done{margin-top:10px;font-weight:600;color:#3fb950}"
        ".ix-ask[data-done] form{opacity:.5;pointer-events:none}"
        "</style>"
    )
    wiring = (
        "<script>(function(){"
        "var root=document.currentScript.closest('.ix-ask');"
        "var form=root.querySelector('form');"
        "function finish(p){window.ixSubmit(p);root.setAttribute('data-done','');"
        "root.querySelector('.ix-done').hidden=false;}"
        "root.querySelectorAll('button[data-ix-value]').forEach(function(b){"
        "b.addEventListener('click',function(){finish({value:b.getAttribute('data-ix-value')});});});"
        "form.addEventListener('submit',function(e){e.preventDefault();"
        "var fd=new FormData(form),o={};fd.forEach(function(v,k){o[k]=v;});"
        "finish(o);});"
        "})();</script>"
    )
    return (
        f'{style}<div class="ix-ask"><div class="ix-prompt">{_escape_html(str(prompt))}</div>'
        f'<form>{controls}</form><div class="ix-done" hidden>✓ sent</div></div>'
        f"{channel.script}{wiring}"
    )


async def ask(
    prompt: str,
    *,
    fields: list[str] | None = None,
    choices: list[str] | None = None,
    title: str | None = None,
    multiline: bool = False,
    submit_label: str = "Submit",
) -> Any:
    """Pop an input window asking the human, wait for their reply, return it.

    Renders a form as a live :class:`Resource` (a dashboard pane, and a floating
    window under ``ix-windows``), blocks until the human submits, closes the
    window, and returns the response shaped to what you asked for:

        await ask("What should I name the branch?")          # -> the typed string
        await ask("Deploy where?", choices=["staging", "prod"])  # -> the chosen string
        await ask("DB creds", fields=["user", "password"])   # -> {"user": ..., "password": ...}

    ``choices`` and ``fields`` are mutually exclusive; pass neither for a single
    free-text answer (``multiline=True`` for a textarea). Because it waits on a
    human it will exceed your ``python_exec`` budget and background the run --
    retrieve the answer in a later call with ``await jobs['<id>']``.

    For HTML beyond a simple form, use :class:`Input` directly.
    """
    if choices is not None and fields is not None:
        raise ValueError("pass choices or fields, not both")
    channel = Input(title=title or prompt)
    html = _ask_form_html(
        channel, prompt, fields=fields, choices=choices, multiline=multiline, submit_label=submit_label
    )
    resource = register_resource(render=lambda: html, id=channel.id, title=title or prompt, kind="input")
    try:
        payload = await channel.recv()
    finally:
        channel.close()
        resource.close()
    # Shape the reply: a single free-text or single-choice answer reads as the
    # bare value; a multi-field form reads as the dict the caller named.
    if fields is None and isinstance(payload, dict) and set(payload) == {"value"}:
        return payload["value"]
    return payload


# Claude Code drops <channel> tag attributes whose keys are not identifiers
# ([A-Za-z0-9_]) -- silently. Validate at the source instead, so a typo'd key is
# a loud ValueError here rather than a missing attribute there.
_META_KEY_RE = re.compile(r"[A-Za-z0-9_]+")


async def notify(content: str, **meta: Any) -> None:
    """Push a channel event into the connected agent session.

    This server is a Claude Code channel (research preview): when the client
    session was launched with the channel enabled (``claude
    --dangerously-load-development-channels server:<name>``), the event lands in
    the agent's context as ``<channel source="<name>" key="val">content</channel>``
    and wakes it. Each keyword becomes a tag attribute (values are stringified)::

        await notify("CI run 1234 failed on main", severity="high", run_id="1234")

    Pass ``resource=<id>`` when the event belongs to an interactive resource, so
    the agent knows to answer with the ``reply`` tool (its text streams back to
    that resource's page).

    Fire-and-forget: delivery is not acknowledged, and a client session running
    WITHOUT the channel enabled drops events silently (Claude Code's documented
    behavior), so never treat a notify as confirmed-read. Keys must be
    identifiers (``[A-Za-z0-9_]``); anything else raises here rather than being
    silently dropped client-side.

    Explicit notify() is a broadcast: an armed watch (``pr_watch``, a slack/CI
    watch loop) must reach its agent regardless of which session runs the
    watcher. Automatic job lifecycle events do NOT go through here -- they are
    addressed to the session that started the job (see
    :func:`_notify_job_finished`), so one session's routine background jobs
    cannot wake another session's agent (issue #2165).
    """
    _queue_channel_event(str(content), meta, session="")


def _queue_channel_event(content: str, meta: dict[str, Any], *, session: str) -> None:
    """Validate and queue one channel event on the store outbox.

    ``session`` is the delivery address: '' broadcasts (every transport pump may
    deliver it), a session id restricts the row to that MCP session's pump. The
    shared write path behind :func:`notify` (broadcast) and
    :func:`_notify_job_finished` (addressed).
    """
    bad = [key for key in meta if not _META_KEY_RE.fullmatch(key)]
    if bad:
        raise ValueError(
            f"notify() meta keys must match [A-Za-z0-9_]+ (Claude Code silently "
            f"drops others); got {bad!r}"
        )
    if _store is None or _store_conn is None:
        raise RuntimeError(
            "notify() needs the server-managed kernel (no store is configured), "
            "so there is no channel to deliver to"
        )
    _store.add_outbox(
        _store_conn,
        content=content,
        meta=json.dumps({key: str(value) for key, value in meta.items()}),
        session=session,
    )


def _server_session() -> str:
    """This server process's own MCP session id, or '' when unmanaged.

    ``IX_MCP_SERVER_SESSION`` is minted per ``ix-mcp serve`` process (see
    ``cli._serve``) and inherited by the kernel: it identifies the one stdio
    client as a session, so jobs it starts can be addressed back to it and only
    it. An embedder driving the runtime without the CLI has no id; '' degrades
    an addressed event to a broadcast, today's pre-#2165 behavior.
    """
    return os.environ.get("IX_MCP_SERVER_SESSION", "")


def _notify_job_finished(job: Job) -> None:
    """Queue the job's terminal lifecycle event, addressed to the session that
    started it.

    Only a backgrounded real cell notifies: a job that finished within its
    budget already returned its summary in the tool reply, and a replay is
    history, not news. The address is the job's own MCP session
    (``job.session``, set for HTTP-transport sessions) falling back to this
    server's session id (the stdio client), so the wake reaches the session
    that started the job and no other -- the dashboard still shows every job
    globally from the executions table (issue #2165).
    """
    if not job.backgrounded or job.kind == "replay":
        return
    _queue_channel_event(
        f"Background job {job.name} finished with status {job.status}.",
        {
            "job_id": job.id,
            "job_name": job.name,
            "status": job.status,
            "topic": job.topic,
        },
        session=job.session or _server_session(),
    )


def _parse_github_time(value: Any) -> datetime.datetime | None:
    if not isinstance(value, str) or not value or value.startswith("0001-"):
        return None
    try:
        return datetime.datetime.fromisoformat(value)
    except ValueError:
        return None


def _format_duration(seconds: float | None) -> str:
    if seconds is None:
        return ""
    seconds = max(0, int(seconds))
    minutes, sec = divmod(seconds, 60)
    hours, minutes = divmod(minutes, 60)
    if hours:
        return f"{hours}h {minutes}m"
    if minutes:
        return f"{minutes}m {sec}s"
    return f"{sec}s"


def _pr_check_duration(check: Mapping[str, Any], now: datetime.datetime) -> str:
    started = _parse_github_time(check.get("startedAt"))
    if started is None:
        return ""
    completed = _parse_github_time(check.get("completedAt"))
    end = completed or now
    return _format_duration((end - started).total_seconds())


def _pr_resource_html(state: Mapping[str, Any]) -> str:
    pr = html_lib.escape(str(state.get("pr") or ""))
    title = html_lib.escape(str(state.get("title") or "PR"))
    url = html_lib.escape(str(state.get("url") or "#"))
    status = html_lib.escape(str(state.get("status") or "starting"))
    merge = html_lib.escape(str(state.get("merge_state") or ""))
    elapsed = html_lib.escape(str(state.get("elapsed") or ""))
    auto = html_lib.escape(str(state.get("auto_merge") or ""))
    error = state.get("error")
    now = datetime.datetime.now(datetime.UTC)
    rows = []
    for check in state.get("checks") or []:
        name = html_lib.escape(str(check.get("name") or check.get("workflowName") or "check"))
        raw_state = str(check.get("conclusion") or check.get("status") or "pending")
        css_state = re.sub(r"[^a-z0-9_-]+", "-", raw_state.lower())
        shown_state = html_lib.escape(raw_state.lower())
        duration = html_lib.escape(_pr_check_duration(check, now))
        rows.append(
            "<tr>"
            f"<td>{name}</td>"
            f"<td><span class=\"state {css_state}\">{shown_state}</span></td>"
            f"<td>{duration}</td>"
            "</tr>"
        )
    if not rows:
        rows.append('<tr><td colspan="3" class="empty">waiting for checks</td></tr>')
    error_html = ""
    if error:
        error_html = f'<div class="error">{html_lib.escape(str(error))}</div>'
    return (
        "<style>"
        "body{margin:0;font:13px ui-sans-serif,system-ui;color:#e5e7eb;background:#111827}"
        ".card{padding:14px;min-width:360px}.top{display:flex;gap:10px;align-items:baseline}"
        "a{color:#93c5fd;text-decoration:none}.title{font-weight:650}.meta{color:#9ca3af;margin:8px 0 12px}"
        "table{border-collapse:collapse;width:100%}td,th{padding:6px 8px;border-top:1px solid #374151;text-align:left}"
        "th{font-size:11px;color:#9ca3af;text-transform:uppercase;letter-spacing:.04em}.state{border-radius:999px;padding:2px 7px;background:#374151}"
        ".completed,.success{background:#064e3b;color:#a7f3d0}.in_progress,.queued,.pending{background:#78350f;color:#fde68a}"
        ".failure,.cancelled,.timed_out,.action_required{background:#7f1d1d;color:#fecaca}.empty{color:#9ca3af;text-align:center}"
        ".error{margin-top:10px;color:#fecaca;background:#7f1d1d;padding:8px;border-radius:6px;white-space:pre-wrap}"
        "</style>"
        "<div class=\"card\">"
        f"<div class=\"top\"><a href=\"{url}\" target=\"_blank\">PR {pr}</a><span class=\"title\">{title}</span></div>"
        f"<div class=\"meta\">{status} {merge} {elapsed} {auto}</div>"
        "<table><thead><tr><th>required action</th><th>state</th><th>time</th></tr></thead>"
        f"<tbody>{''.join(rows)}</tbody></table>{error_html}</div>"
    )


async def watch_pr(
    pr: str | int,
    *,
    cwd: str | None = None,
    auto_merge: bool = True,
    merge_method: str = "squash",
    delete_branch: bool = True,
    interval: float = 15.0,
    timeout: float = 3600.0,
) -> dict[str, Any]:
    """Watch a GitHub PR, mirror required checks as a resource, and optionally enable auto merge."""
    clean_pr = str(pr).strip()
    if not clean_pr:
        raise ValueError("pr must be a number, URL, or branch understood by gh")
    if merge_method not in {"merge", "squash", "rebase"}:
        raise ValueError("merge_method must be merge, squash, or rebase")
    safe_id = re.sub(r"[^A-Za-z0-9._-]+", "-", clean_pr).strip("-") or uuid.uuid4().hex[:8]
    state: dict[str, Any] = {
        "pr": clean_pr,
        "title": "",
        "url": "",
        "status": "starting",
        "merge_state": "",
        "checks": [],
        "auto_merge": "",
        "error": "",
        "elapsed": "",
    }
    started = time.time()
    resource = register_resource(
        render=lambda: _pr_resource_html(state),
        id=f"pr-{safe_id}",
        title=f"PR {clean_pr}",
        kind="pr",
        alive=lambda: state.get("status") not in {"merged", "closed", "failed", "timed out"},
    )
    import nu as nu_call

    async def run_nu(code: str) -> Any:
        return await nu_call(
            code,
            cwd=cwd,
            env={"PR": clean_pr},
            timeout=60,
        )

    async def refresh() -> dict[str, Any]:
        out = await run_nu(
            'gh pr view $env.PR --json number,title,state,mergeStateStatus,statusCheckRollup,'
            'url,autoMergeRequest,isDraft,reviewDecision | complete | get stdout | from json'
        )
        row = out.to_dicts()[0]
        checks = row.get("statusCheckRollup") or []
        title = row.get("title") or f"PR {row.get('number') or clean_pr}"
        state.update(
            {
                "pr": row.get("number") or clean_pr,
                "title": title,
                "url": row.get("url") or "",
                "status": str(row.get("state") or "").lower(),
                "merge_state": row.get("mergeStateStatus") or "",
                "checks": checks,
                "auto_merge": "auto merge on" if row.get("autoMergeRequest") else "auto merge off",
                "elapsed": _format_duration(time.time() - started),
                "error": "",
            }
        )
        return row

    if auto_merge:
        flag = f"--{merge_method}"
        delete = "--delete-branch" if delete_branch else ""
        merge = await run_nu(f"gh pr merge $env.PR --auto {flag} {delete} | complete")
        if int(merge["exit_code"][0]) != 0:
            state["error"] = str(merge["stderr"][0] or merge["stdout"][0])

    last: dict[str, Any] = {}
    while True:
        last = await refresh()
        checks = last.get("statusCheckRollup") or []
        failures = [
            check
            for check in checks
            if check.get("conclusion") in {"FAILURE", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED"}
        ]
        if last.get("state") != "OPEN":
            state["status"] = "merged" if last.get("state") == "MERGED" else "closed"
            resource.close()
            await notify(f"PR {clean_pr} finished with state {last.get('state')}", resource=resource.id, pr=clean_pr)
            return {"state": last.get("state"), "url": last.get("url"), "checks": len(checks)}
        if failures:
            state["status"] = "failed"
            state["error"] = "One or more required actions failed."
            resource.close()
            await notify(f"PR {clean_pr} has failing checks", resource=resource.id, pr=clean_pr)
            return {"state": "failed", "url": last.get("url"), "failures": failures}
        if time.time() - started > timeout:
            state["status"] = "timed out"
            resource.close()
            await notify(f"PR {clean_pr} watch timed out", resource=resource.id, pr=clean_pr)
            return {"state": "timed out", "url": last.get("url"), "checks": len(checks)}
        await asyncio.sleep(interval)


@dataclasses.dataclass
class _Cell:
    id: str
    title: str
    outputs: list  # rendered mime bundles, newest render


class Cells:
    """The curated presentation pane: what the agent chooses to PRESENT.

    Where ``jobs`` is every run (the dashboard's executions column shows them
    all) and ``resources`` is every live view, ``cells`` is the agent's own
    highlight reel. Fill it with the most important results and the dashboard
    renders them as a third pane, in order, so a session reads as a live,
    informative summary instead of a raw log. Each value is rendered the way the
    dashboard renders any result: a :class:`Result`'s ``user_html``, a polars
    DataFrame as a table, a matplotlib figure as an image, anything else as its
    display HTML or repr.

        cells.add(df, title="latency by host")   # append a titled cell, returns its id
        cells.add(fig, title="throughput")
        cells.set(0, df2)                         # replace a cell in place (id or index)
        cells.remove(0)                           # drop one (id or index)
        cells.clear()                             # start the presentation over

    Adding with an ``id`` that already exists replaces that cell, so a loop can
    keep one cell updated in place (a live metric) rather than appending forever.
    """

    def __init__(self) -> None:
        self._items: list[_Cell] = []
        self._rev = 0
        self._synced = -1

    def _render(self, value: Any, title: str | None) -> list:
        bundle = _result_bundle(value)
        if bundle is not None and bundle.get("data"):
            return [bundle]
        return [{"data": {"text/plain": _safe_repr(value)}, "metadata": {}}]

    def _find(self, key: int | str) -> int:
        """Resolve an int index or a string id to a list index, or -1."""
        if isinstance(key, int):
            return key if -len(self._items) <= key < len(self._items) else -1
        for i, cell in enumerate(self._items):
            if cell.id == key:
                return i
        return -1

    def add(self, value: Any, *, title: str | None = None, id: str | None = None) -> str:
        """Append a cell (or replace the one with ``id``); return its id."""
        outputs = self._render(value, title)
        idx = self._find(id) if id is not None else -1
        if idx >= 0:
            self._items[idx].title = title or self._items[idx].title
            self._items[idx].outputs = outputs
            cid = self._items[idx].id
        else:
            cid = id or uuid.uuid4().hex[:8]
            self._items.append(_Cell(cid, title or "", outputs))
        self._rev += 1
        return cid

    # append is the list-flavoured spelling of add.
    append = add

    def set(self, key: int | str, value: Any, *, title: str | None = None) -> str:
        """Replace the cell at ``key`` (an int index or a string id) in place."""
        idx = self._find(key)
        if idx < 0:
            raise KeyError(f"no cell {key!r}")
        self._items[idx].outputs = self._render(value, title)
        if title is not None:
            self._items[idx].title = title
        self._rev += 1
        return self._items[idx].id

    def remove(self, key: int | str) -> None:
        """Drop the cell at ``key`` (an int index or a string id)."""
        idx = self._find(key)
        if idx < 0:
            raise KeyError(f"no cell {key!r}")
        del self._items[idx]
        self._rev += 1

    def clear(self) -> None:
        """Empty the presentation."""
        if self._items:
            self._items = []
            self._rev += 1

    def __setitem__(self, key: int | str, value: Any) -> None:
        if isinstance(key, str):
            self.add(value, id=key)
        else:
            self.set(key, value)

    def __getitem__(self, key: int | str) -> _Cell:
        idx = self._find(key)
        if idx < 0:
            raise KeyError(f"no cell {key!r}")
        return self._items[idx]

    def __len__(self) -> int:
        return len(self._items)

    def __iter__(self) -> Any:
        return iter(self._items)

    def __repr__(self) -> str:
        titles = ", ".join(c.title or c.id for c in self._items)
        return f"<Cells [{len(self._items)}]{': ' + titles if titles else ''}>"

    def _sync(self) -> None:
        """Mirror the current presentation to the store when it has changed.

        Declarative: ``cells`` is the source of truth and the store is a derived
        view, so each change replaces the table contents wholesale (the set is
        small). Guarded by a revision counter so an unchanged presentation costs
        nothing per flush tick."""
        if self._rev == self._synced or _store is None or _store_conn is None:
            return
        rows = [
            {"id": c.id, "title": c.title, "position": i, "outputs": c.outputs}
            for i, c in enumerate(self._items)
        ]
        with contextlib.suppress(Exception):  # best-effort: store write must not raise into user code
            _store.replace_cells(_store_conn, rows)
            self._synced = self._rev


cells = Cells()


class Session:
    """This MCP session — how the dashboard groups your runs.

    Every ``python_exec`` run on this kernel belongs to one session: one MCP
    client (a Claude Code window, an editor) talking to one ``ix-mcp serve``
    process. The dashboard's session selector lists each live session by its
    ``name``, so naming yours is the first thing to do — a human watching several
    agents at once can then tell them apart at a glance:

        session.name = "refactor the auth flow"   # retitle this session
        session.name                               # the label shown now

    Until you set a name, the label defaults to the connecting client and this
    kernel's working directory (e.g. ``claude-code · index``), which is fine for
    one agent but ambiguous once several share a repo — so set it.
    """

    def __init__(self) -> None:
        self._name = ""  # explicit, user-set via `session.name = ...`
        self._client = ""  # the connecting MCP client's reported identity
        self._workdir = ""  # this kernel's cwd basename, for the default label
        self._topic = ""  # current fold group for runs in this session
        self._rev = 0
        self._synced = -1

    @property
    def name(self) -> str:
        """The effective label: the user-set name, else client · workdir."""
        if self._name:
            return self._name
        parts = [p for p in (self._client, self._workdir) if p]
        return " · ".join(parts) or "session"

    @name.setter
    def name(self, value: str) -> None:
        self._name = (value or "").strip()
        self._rev += 1

    @property
    def topic(self) -> str:
        """The current dashboard fold group for future runs."""
        return self._topic or "unfiled"

    @topic.setter
    def topic(self, value: str) -> None:
        self._topic = " ".join((value or "").split())
        self._rev += 1

    @property
    def client(self) -> str:
        """The connecting MCP client's reported identity (read-only)."""
        return self._client

    def _set_client(self, client: str) -> None:
        """Record the connecting client's identity. Called once by the server when
        the MCP client identifies itself at ``initialize`` — not user-facing."""
        client = (client or "").strip()
        if client and client != self._client:
            self._client = client
            self._rev += 1

    def __repr__(self) -> str:
        tail = f" · {self._client}" if self._client and self._client != self.name else ""
        topic = f" topic={self.topic!r}" if self._topic else ""
        return f"<Session {self.name!r}{tail}{topic}>"

    def _sync(self) -> None:
        """Mirror the session label to the store when it has changed, so the
        dashboard's selector picks it up. Best-effort, like ``cells._sync``."""
        if self._rev == self._synced or _store is None or _store_conn is None:
            return
        with contextlib.suppress(Exception):  # best-effort: store write must not raise into user code
            _store.set_session(_store_conn, name=self.name, client=self._client)
            self._synced = self._rev


session = Session()


# AST node types that open their own scope: a `yield` (or a name binding) inside
# one of these belongs to that inner scope, not the cell's top level.
_NESTED_SCOPES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.ClassDef)


def _has_toplevel_yield(nodes: Any) -> bool:
    """True if a ``yield``/``yield from`` appears at the cell's own top level
    (not inside a nested def, lambda, or class). Such a cell is run as a
    generator: each ``yield Result(...)`` streams a result to both the human and
    the model, instead of the cell ending with a single trailing Result."""
    for node in nodes:
        if isinstance(node, (ast.Yield, ast.YieldFrom)):
            return True
        if isinstance(node, _NESTED_SCOPES):
            continue  # its own scope: a yield in there is that scope's, not ours
        if _has_toplevel_yield(ast.iter_child_nodes(node)):
            return True
    return False


def _compile(code: str, filename: str) -> tuple[str, types.CodeType]:
    """Compile a cell, returning ``(mode, code_obj)``.

    ``mode == "gen"`` for a cell that yields at top level (run as an async
    generator; see :func:`_compile_generator`). Otherwise ``mode == "expr"``: the
    classic path that allows top-level ``await`` and captures a trailing
    expression into ``__ix_result`` (REPL-style) so the cell has a result like a
    notebook cell does."""
    tree = ast.parse(code, filename, "exec")
    if _has_toplevel_yield(tree.body):
        return ("gen", _compile_generator(code, filename))
    if tree.body and isinstance(tree.body[-1], ast.Expr):
        last = tree.body[-1]
        assign = ast.Assign(targets=[ast.Name(id="__ix_result", ctx=ast.Store())], value=last.value)
        ast.copy_location(assign, last)
        tree.body[-1] = assign
        ast.fix_missing_locations(tree)
    return ("expr", compile(tree, filename, "exec", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT))


def _compile_generator(code: str, filename: str) -> types.CodeType:
    """Compile a yielding cell as ``async def __ix_cell__()`` whose top-level
    names stay in the shared namespace.

    Wrapping the body in an async function makes top-level ``yield`` and ``await``
    legal; declaring every name the body binds ``global`` keeps assignments
    persistent across calls, exactly like a normal module-level cell. The set of
    bound names is read from Python's own scope analysis (the compiled function's
    locals and cellvars), so it is precisely the names that would otherwise be
    function-locals, without re-deriving Python's scoping rules by hand. The user
    statements keep their original line numbers (only an enclosing def is added),
    so tracebacks still point at the cell's real lines."""
    user = ast.parse(code, filename, "exec")
    shell = ast.parse("async def __ix_cell__():\n    pass\n", filename, "exec")
    func = shell.body[0]
    # The shell source is a single `async def`, so body[0] is that function def;
    # narrow it so its `.body` is statically known.
    assert isinstance(func, ast.AsyncFunctionDef)
    func.body = user.body  # the cell's own statements, original line numbers intact
    ast.fix_missing_locations(shell)
    probe = compile(shell, filename, "exec")
    cell_code = next(
        c for c in probe.co_consts if isinstance(c, types.CodeType) and c.co_name == "__ix_cell__"
    )
    # co_varnames + co_cellvars are the names this function binds; making them
    # global is what turns "function locals" back into "notebook globals". Names
    # like ".0" (comprehension internals) live in their own code objects, never
    # here, but filter the dotted ones defensively.
    names = [n for n in (cell_code.co_varnames + cell_code.co_cellvars) if not n.startswith(".")]
    if names:
        func.body.insert(0, ast.parse("global " + ", ".join(names)).body[0])
        ast.fix_missing_locations(shell)
    return compile(shell, filename, "exec")


def _merge_stdout(job: Job, result: Result) -> Result:
    """Jupyter shows a cell's stdout AND its final value; so do we. When a cell
    both printed and ended with a bare (non-Result) expression, prepend the
    captured stdout to the model text and the human view, clipped like any other
    large output (the full capture stays pageable as ``jobs['<id>'].output``).
    Explicit Results are exempt: the author already declared both views."""
    printed = job.output
    if not printed.strip():
        return result
    body = printed
    if len(body) > _AUTO_RESULT_CHARS:
        body = body[-_AUTO_RESULT_CHARS:] + (
            f"\n... [stdout clipped to the last {_AUTO_RESULT_CHARS} of "
            f"{len(printed)} chars; page jobs['{job.id}'].output]"
        )
    text = _strip_ansi(body)
    if not text.endswith("\n"):
        text += "\n"
    merged = Result(
        user_html=f'<pre class="ix-result">{_ansi_to_html(body)}</pre>' + result.user_html,
        llm_result=text + (result.llm_result or ""),
        llm_images=result.llm_images,
    )
    # Merging stdout replaces the WRAPPER, not the value: keep the trailing
    # expression's original reachable (`jobs['<id>'].result.value` / `[i, j]`).
    merged.value = result.value
    return merged


# Cap on the stdout an auto-returned Result (see _auto_result) hands the model.
# A chatty print-only cell keeps its most recent slice inline; the full capture
# stays pageable as jobs['<id>'].output, exactly like any other large output.
_AUTO_RESULT_CHARS = 20_000


def _auto_result(job: Job) -> Result:
    """The Result for a cell whose last statement evaluated to None (an
    assignment, a bare ``print()``, a side-effecting call): its captured stdout,
    or a quiet ok when it printed nothing -- the same thing a notebook shows."""
    printed = job.output
    if not printed.strip():
        return Result.ok("done (cell returned no value)")
    body = printed
    if len(body) > _AUTO_RESULT_CHARS:
        body = body[-_AUTO_RESULT_CHARS:] + (
            f"\n... [stdout clipped to the last {_AUTO_RESULT_CHARS} of "
            f"{len(printed)} chars; page jobs['{job.id}'].output]"
        )
    return Result(
        user_html=f'<pre class="ix-result">{_ansi_to_html(body)}</pre>',
        llm_result=_strip_ansi(body),
    )


def _display_result(result: Result) -> None:
    """Show one yielded Result to both audiences. The IPython display goes onto
    the running job's captured outputs (the dashboard) and out on iopub (the
    model's tool result), the same path the trailing Result takes \u2014 so a
    yielding cell needs no separate plumbing."""
    from IPython.display import display

    with contextlib.suppress(Exception):  # best-effort: formatter failure must not abort the run
        display(result)


def _is_displayable(value: Any) -> bool:
    """True if a value carries its own rich rendering: an IPython rich repr
    (a polars DataFrame, a ``view.Code``, ...), an htpy-style ``__html__``, or a
    figure/image that renders through a registered formatter. Plain scalars,
    ``str``/``bytes``, and the container types (dict/list/tuple/set) are not
    (they render through ``Result.of``'s text paths instead).
    """
    if _image_bytes_mime(value) is not None:
        # Raw image bytes (a screenshot) know how to render: as an inline image.
        return True
    if value is None or isinstance(
        value, (str, bytes, bool, int, float, complex, dict, list, tuple, set, frozenset)
    ):
        return False
    for attr in (
        "_repr_html_", "_repr_png_", "_repr_jpeg_", "_repr_svg_",
        "_repr_markdown_", "_repr_latex_", "_repr_mimebundle_", "__html__",
    ):
        if callable(getattr(value, attr, None)):
            return True
    # Figures/axes/images render via a registered formatter, not a method.
    module = type(value).__module__ or ""
    return module.startswith(("matplotlib", "PIL"))


def _current_line(job: Job) -> int | None:
    """The cell line ``job`` is executing right now, or None.

    Read off the suspended coroutine chain: starting from the cell's own
    coroutine (or async generator), follow what each frame is awaiting and keep
    the deepest frame that belongs to this job's pseudo-file (``<job id>``).
    That is exactly the line a human would point at: the cell line whose await
    is in flight, even when the wait itself is deep inside a library. Costs one
    attribute walk (no tracing), so the flusher can sample it every tick. None
    for a purely synchronous cell (it has no suspended frame to read; it also
    holds the loop, so nothing could repaint anyway)."""
    obj = job._aobj
    if obj is None or not job.running():
        return None
    target = f"<job {job.id}>"
    line = None
    for _ in range(128):  # defensive bound; await chains are short in practice
        frame = (
            getattr(obj, "cr_frame", None)
            or getattr(obj, "ag_frame", None)
            or getattr(obj, "gi_frame", None)
        )
        if frame is None:
            break
        if frame.f_code.co_filename == target:
            line = frame.f_lineno
        obj = (
            getattr(obj, "cr_await", None)
            or getattr(obj, "ag_await", None)
            or getattr(obj, "gi_yieldfrom", None)
        )
        if obj is None:
            break
    return line


def _user_traceback(exc: BaseException) -> str:
    """``exc`` formatted with the kernel's own plumbing frames cut off.

    The frames above the cell (``_runner``, the ``exec``/``eval`` trampoline)
    are noise to both audiences, so the traceback starts at the first frame in
    a ``<job ...>`` pseudo-file -- the cell itself -- like a notebook's. A
    SyntaxError never enters the cell's frame, so it falls back to the
    exception-only form, which already carries the offending line and caret;
    anything else without a user frame keeps the full traceback."""
    tb = exc.__traceback__
    while tb is not None and not tb.tb_frame.f_code.co_filename.startswith("<job "):
        tb = tb.tb_next
    if tb is not None:
        return "".join(traceback.format_exception(type(exc), exc, tb))
    if isinstance(exc, SyntaxError):
        return "".join(traceback.format_exception_only(type(exc), exc))
    return "".join(traceback.format_exception(type(exc), exc, exc.__traceback__))


def _error_line(exc: BaseException, job: Job) -> int | None:
    """The cell line the failure was raised on: a SyntaxError's reported line,
    else the deepest traceback frame inside this job's own pseudo-file (the
    cell line whose statement failed, even when the raise happened in a
    library below it)."""
    target = f"<job {job.id}>"
    if isinstance(exc, SyntaxError) and exc.filename == target:
        return exc.lineno
    line = None
    tb = exc.__traceback__
    while tb is not None:
        if tb.tb_frame.f_code.co_filename == target:
            line = tb.tb_lineno
        tb = tb.tb_next
    return line


def _typecheck_enabled() -> bool:
    """Whether per-cell type checking runs. Default ON; the escape hatch is the
    ``IX_MCP_TYPECHECK`` env var (``0``/``false``/``no``/``off`` disables it) or,
    when a Config is set, its ``typecheck`` flag. The env var wins if present, so
    a session can toggle it without a config rebuild."""
    raw = os.environ.get("IX_MCP_TYPECHECK")
    if raw is not None:
        return raw.strip().lower() not in ("0", "false", "no", "off")
    try:
        from . import config as _config_mod

        return _config_mod.config().typecheck
    except Exception:
        # No config set (one-shot eval, bare kernel, tests): default on.
        return True


async def _runner(job: Job, ns: dict) -> None:
    token = _ix_current.set(job)
    # (Re-)arm the task-failure watch on the loop actually running jobs.
    # `install()` may have run in a sync context where `get_event_loop()`
    # handed back a dormant default loop that is not this one; installing
    # here (idempotent, sentinel-guarded) guarantees every task a cell spawns
    # is watched regardless of how the kernel was embedded.
    _install_task_failure_watch(asyncio.get_running_loop())
    if _store is not None and _store_conn is not None:
        with contextlib.suppress(Exception):  # best-effort: store write must not abort the job
            _store.start(
                _store_conn,
                id=job.id,
                name=job.name,
                code=job.code,
                started_at=job.started,
                budget=job.budget,
                kind=job.kind,
                topic=job.topic,
            )
    try:
        # Static type check BEFORE running (default on; IX_MCP_TYPECHECK=0 or the
        # `typecheck` config flag disables it). A type error is caught here and
        # returned as the result -- the cell never executes -- so the agent fixes
        # it and retries instead of hitting it three lines into a side-effecting
        # cell. The checker's own failures never block a cell (see typecheck.check).
        # Replays are exempt: a session reopen re-runs cells that ALREADY executed
        # successfully (kind="replay", see store.replayable), and blocking one on
        # a checker finding would silently drop its bindings from the restored
        # namespace -- the check already had its chance when the cell first ran.
        if job.kind != "replay" and _typecheck_enabled():
            verdict = await typecheck.check(job.code, ns)
            if not verdict.ok:
                # Record it like any other failed cell: `error` (the dashboard's
                # error highlight, and what the summary carries), and the report as
                # the result so the model's reply is the diagnostic to fix. No
                # `_exc`: the agent should read and fix the diagnostic, not have
                # `await jobs['<id>']` raise an opaque error.
                job.status = "error"
                job.error = verdict.report
                job._result = Result.of(verdict.report)
                return
        # Compile inside the runner so a SyntaxError is recorded as a job error
        # (status + traceback in the store/dashboard) instead of escaping __ix_run.
        mode, code_obj = _compile(job.code, f"<job {job.id}>")
        ns.pop("__ix_result", None)
        if mode == "gen":
            # A yielding cell streams results: drain the async generator and
            # display each yielded value as it is produced, so it reaches the
            # human (the job's captured outputs) and the model (iopub). Any
            # value can be yielded; a non-Result renders through Result.of,
            # exactly like a trailing expression.
            exec(code_obj, ns)  # noqa: S102 -- intentional: executing compiled user cell code
            agen = ns.pop("__ix_cell__")()
            job._aobj = agen  # sampled by _current_line while suspended
            emitted = 0
            async for item in agen:
                _display_result(item if isinstance(item, Result) else Result.of(item))
                emitted += 1
            if emitted == 0:
                # A generator cell that yielded nothing still ran: report it
                # like a None-valued cell (its stdout, or a quiet ok).
                _display_result(_auto_result(job))
            job.status = "done"
            # The results were displayed as they streamed; there is no single
            # trailing value to return.
            job._result = None
        else:
            maybe = eval(code_obj, ns)  # noqa: S307 -- intentional: evaluating compiled user cell code
            if inspect.iscoroutine(maybe):
                job._aobj = maybe  # sampled by _current_line while suspended
                await maybe
            value = ns.pop("__ix_result", None)
            if value is None:
                # A cell whose last statement evaluated to None -- an assignment,
                # a bare print(), a side-effecting call -- returns its captured
                # stdout (or a quiet ok), so a print-only cell reports what it
                # printed.
                value = _auto_result(job)
            elif isinstance(value, Result):
                # An explicit Result is the author's full statement of both
                # views; stdout stays out of it (page jobs['<id>'].output).
                pass
            else:
                # Jupyter semantics: the last expression IS the result, whatever
                # its type. Result.of renders any value (rich types richly,
                # scalars/strings/containers as their natural text), and stdout
                # the cell printed along the way rides with it.
                value = _merge_stdout(job, Result.of(value))
            job._result = value
            job.status = "done"
    except asyncio.CancelledError:
        job.status = "cancelled"
        raise
    except KeyboardInterrupt as _kexc:
        job.status = "error"
        if job.interrupted_by_watchdog:
            # The server's wedge watchdog (SIGUSR2, fired after config.wedge_grace)
            # raised this: a synchronous call blocked the event loop past the
            # budget. Record a crisp, actionable message instead of a bare traceback.
            job.error = (
                "Interrupted: this cell exceeded its budget while blocking the "
                "kernel's event loop with a synchronous call (subprocess.run, "
                "time.sleep, requests, a long CPU op), which freezes every job. Wrap "
                "it in `await asyncio.to_thread(...)` or use an async API, and run "
                "anything slow as a background job."
            )
            # Keep the interrupt as the job's exception (with the actionable
            # message) so `await jobs['<id>']` re-raises rather than yielding None.
            job._exc = _kexc
            _kexc.args = (job.error,)
        else:
            # The user's own code raised KeyboardInterrupt; keep its real
            # traceback (trimmed to the cell's frames) and the failing line.
            job.error = _user_traceback(_kexc)
            job.error_line = _error_line(_kexc, job)
            job._exc = _kexc
        job._exc_tb = _kexc.__traceback__
        job._append(job.error)
    except (Exception, SystemExit) as _exc:
        # Isolate user code from the kernel: a job's SyntaxError, exception, or
        # even sys.exit()/exit() becomes a failed job (traceback captured) instead
        # of escaping the task and tearing down the shared kernel session.
        # asyncio.CancelledError is BaseException, not caught here, so cooperative
        # cancellation (handled above) still propagates.
        job.status = "error"
        # Trim the kernel's plumbing frames so the traceback starts at the cell,
        # and record the failing cell line for the dashboard's error highlight.
        tb = _user_traceback(_exc)
        job.error_line = _error_line(_exc, job)
        hint = _type_error_hint(_exc) if isinstance(_exc, TypeError) else ""
        job.error = tb + hint
        # Keep the exception object itself, so `await jobs['<id>']` re-raises it
        # (type + message + the cell's own traceback) instead of yielding None.
        job._exc = _exc
        job._exc_tb = _exc.__traceback__
        job._append(job.error)
    finally:
        job.ended = time.time()
        _ix_current.reset(token)
        _persist_final(job)
        _mark_snapshot_dirty()
        with contextlib.suppress(Exception):  # best-effort wake; the job row is already persisted
            _notify_job_finished(job)


def _persist_final(job: Job) -> None:
    if _store is None or _store_conn is None:
        return
    # Record this run's name references first, so the snapshot below already shows
    # the just-finished job among each name's assigned_in/used_in.
    _record_refs(job)
    with contextlib.suppress(Exception):  # best-effort: persist final status; must not raise during cleanup
        result_repr = None if job._result is None else _safe_repr(job._result)
        _store.finish(
            _store_conn,
            id=job.id,
            status=job.status,
            ended_at=job.ended or time.time(),
            output=job.output,
            result=result_repr,
            error=job.error,
            error_line=job.error_line,
            outputs=_job_outputs(job),
            bindings=_cell_bindings(job),
            namespace=_namespace_snapshot(job),
        )


async def _spawn_runner(job: Job, aw: Awaitable[Any]) -> None:
    """:func:`_runner`'s sibling for :meth:`Jobs.spawn`: there is no cell code
    to typecheck or compile -- just await the registered awaitable -- but the
    rest of the lifecycle is identical: prints captured under the job, the value
    wrapped like a trailing expression, the same persistence, and the same
    completion notification a backgrounded cell sends."""
    token = _ix_current.set(job)
    _install_task_failure_watch(asyncio.get_running_loop())
    if _store is not None and _store_conn is not None:
        with contextlib.suppress(Exception):  # best-effort: store write must not abort the job
            _store.start(
                _store_conn,
                id=job.id,
                name=job.name,
                code=job.code,
                started_at=job.started,
                budget=job.budget,
                kind=job.kind,
                topic=job.topic,
            )
    try:
        value = await aw
        if value is None:
            # Same contract as a None-valued cell: the captured stdout (or a
            # quiet ok) is the result, so a print-only awaitable reports what
            # it printed.
            result = _auto_result(job)
        elif isinstance(value, Result):
            # An explicit Result is the author's full statement of both views;
            # stdout stays out of it (page jobs['<id>'].output), like a cell's.
            result = value
        else:
            result = _merge_stdout(job, Result.of(value))
        job._result = result
        job.status = "done"
    except asyncio.CancelledError:
        job.status = "cancelled"
        # `job.cancel()` cancels THIS runner; a pre-created Task/Future keeps
        # running unless the cancellation is forwarded to it. Forward it, so
        # cancelling the job cancels the work whatever shape was registered.
        if isinstance(aw, asyncio.Future):
            aw.cancel()
        raise
    except (Exception, KeyboardInterrupt, SystemExit) as _exc:
        # Isolate like _runner: a failed awaitable becomes a failed job
        # (traceback captured, `await jobs['<id>']` re-raises) instead of
        # escaping the task and dying as an unretrieved background failure.
        job.status = "error"
        job.error = _user_traceback(_exc)
        job._exc = _exc
        job._exc_tb = _exc.__traceback__
        job._append(job.error)
    finally:
        job.ended = time.time()
        _ix_current.reset(token)
        _persist_final(job)
        _mark_snapshot_dirty()
        # Spawned jobs are backgrounded by construction, so completion always
        # notifies (the suppress mirrors _runner: a session without the channel
        # has nothing to deliver to, and that must not fail the job's cleanup).
        with contextlib.suppress(Exception):
            await notify(
                f"Background job {job.name} finished with status {job.status}.",
                job_id=job.id,
                job_name=job.name,
                status=job.status,
                topic=job.topic,
            )


def _cell_bindings(job: Job) -> dict:
    """The live value each of the cell's identifiers is bound to, snapshotted now
    that the job has finished. Read off the namespace the code actually ran in
    (the job's own -- per-session or shared), so the dashboard can show inlay
    hints and hover values that reflect the actual objects. Best-effort: a
    failure here just means no hints."""
    ns = job._ns if job._ns is not None else _shared_ns()
    try:
        from .introspect import cell_bindings

        return cell_bindings(job.code, ns)
    except Exception:
        return {}


# Per-name provenance for the namespace pane: which runs bound or referenced each
# name. Accumulated across the whole session (the pane itself is rebuilt every job
# finish, but a variable's references span every run that touched it). Each list is
# kept most-recent-last, deduped, and capped so a name touched by hundreds of runs
# stays bounded. Reset by install() so a fresh session starts clean.
_MAX_REFS_PER_NAME = 25
_name_refs: dict[str, dict[str, list[str]]] = {}


def _push_ref(name: str, key: str, job_id: str) -> None:
    """Append ``job_id`` under ``_name_refs[name][key]`` (``"assigned_in"`` or
    ``"used_in"``), most-recent-last, deduped and capped to ``_MAX_REFS_PER_NAME``."""
    entry = _name_refs.setdefault(name, {"assigned_in": [], "used_in": []})
    lst = entry[key]
    if job_id in lst:
        lst.remove(job_id)
    lst.append(job_id)
    if len(lst) > _MAX_REFS_PER_NAME:
        del lst[:-_MAX_REFS_PER_NAME]


def _record_refs(job: Job) -> None:
    """Record this run as an assigner/user of each name its source binds/references,
    so the namespace pane can link every variable back to the runs behind it.
    Source-based (see :func:`introspect.binding_names`): correct even when many
    background jobs mutate one shared namespace concurrently, and free (no per-access
    kernel hook). Best-effort: a failure here just means a name shows no references.

    Only a run that finished cleanly (``status == "done"``) contributes. Source
    attribution cannot tell an assignment that executed from one a failed/cancelled
    run never reached (``x = undefined()`` raises before binding ``x``), so crediting
    such a run would claim it set a value it did not. We skip it entirely: a run that
    half-bound names before erroring loses that attribution (a name may show no
    ``assigned_in``), which is the honest trade — under-attribute rather than
    mislead."""
    if job.status != "done":
        return
    with contextlib.suppress(Exception):  # best-effort: attribution failure must not abort
        from .introspect import binding_names

        assigned, used = binding_names(job.code)
        for name in assigned:
            _push_ref(name, "assigned_in", job.id)
        for name in used:
            _push_ref(name, "used_in", job.id)


def _namespace_snapshot(job: Job) -> list:
    """Every user-bound name in the job's namespace, described for the dashboard's
    namespace pane. Stored with each finished run; the newest is the live
    namespace. Each row also carries the runs that assigned/used the name (see
    :data:`_name_refs`). Best-effort: a failure here just means no namespace pane."""
    ns = job._ns if job._ns is not None else _shared_ns()
    try:
        from .introspect import namespace_rows

        return namespace_rows(_namespace_candidates(ns), refs=_name_refs)
    except Exception:
        return []


def _safe_repr(value: Any) -> str:
    try:
        return repr(value)
    except Exception:
        return f"<unreprable {type(value).__name__}>"


# Terminal-escape (ANSI) handling, shared by the Result renderer here and the
# bundled `sh` helper (which imports these). A CLI emits not only SGR color but
# OSC-8 hyperlinks, charset resets, and cursor moves; captured output must reach
# the model as readable text with all of that removed, and reach the human as
# that same text with its color rendered to HTML. One implementation, two
# consumers. Order matters: the string-terminated families (OSC/DCS) come before
# the single-final forms so an introducer is never half-matched.
_ANSI = re.compile(
    r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"  # OSC string, BEL- or ST-terminated
    r"|\x1b[P^_X][^\x1b]*\x1b\\"  # DCS/PM/APC/SOS string, ST-terminated
    r"|\x1b\[[0-9;?]*[ -/]*[@-~]"  # CSI (color, cursor, mode)
    r"|\x1b[()*+#%][@-~]"  # charset designation / selection (e.g. ESC ( B)
    r"|\x1b[@-Z\\-_a-z=>]"  # remaining solo Fe/Fs escapes (RIS, keypad, ...)
)


def _strip_ansi(text: str) -> str:
    """``text`` with every terminal escape sequence removed."""
    return _ANSI.sub("", text)


def _ansi_to_html(text: str) -> str:
    """Render the ANSI SGR color in ``text`` to inline-styled, HTML-escaped
    markup. Falls back to escaped, escape-stripped text when the ``ansi2html``
    converter is unavailable (so it never leaks raw control bytes)."""
    try:
        from ansi2html import Ansi2HTMLConverter
    except Exception:
        return _escape_html(_strip_ansi(text))
    return Ansi2HTMLConverter(inline=True, scheme="osx", dark_bg=True).convert(text, full=False)


# How many rows of a DataFrame the model-facing text carries. The human's HTML
# table is unaffected (it renders the whole frame, paged); this only bounds the
# NUON handed back to the agent so a million-row frame cannot flood its context.
_DF_LLM_ROWS = 200


def _is_polars_df(value: Any) -> bool:
    """True for a polars DataFrame, by duck typing.

    runtime.py stays import-light (polars is the user's to bring), and Nix-built
    wheels may expose extension-backed classes whose module is not simply
    ``polars.*``.
    """
    return (
        hasattr(value, "iter_rows")
        and hasattr(value, "columns")
        and hasattr(value, "dtypes")
        and hasattr(value, "shape")
        and hasattr(value, "height")
    )


def _frame_view(value: Any) -> Any:
    """A non-DataFrame value that opts into the table protocol by exposing
    ``_ix_to_frame_()`` returning a polars DataFrame. Returns that frame, else
    None. Lets a rich result type
    render as the styled table for the human and compact NUON for the model,
    instead of falling back to its one-line summary repr."""
    hook = getattr(value, "_ix_to_frame_", None)
    if hook is None:
        return None
    try:
        frame = hook()
    except Exception:
        return None
    return frame if _is_polars_df(frame) else None


def _df_llm_text(df: Any) -> str:
    """A polars DataFrame as compact NUON for the model.

    The shape + dtype header orients the reader; the body is a Nushell table
    literal with headers listed once. Values are never width-truncated (only the
    row count is bounded by ``_DF_LLM_ROWS``), so the agent reads the real data
    instead of a boxed repr. A 1x1 string frame is the exception: its body is
    the string verbatim, not a one-cell table.
    """
    try:
        rows, cols = df.shape
        head = df.head(_DF_LLM_ROWS)
        schema = ", ".join(f"{name}:{dtype}" for name, dtype in zip(df.columns, df.dtypes, strict=False))
        if rows == 1 and cols == 1 and isinstance(only := head.to_dicts()[0][head.columns[0]], str):
            # A 1x1 string frame is text, not a table -- `await nu("^cat f.toml")`
            # or `^git remote -v` frames the whole capture as one scalar cell.
            # Hand the model the string verbatim (real newlines, terminal escapes
            # stripped: the plain-str treatment), not one JSON-escaped NUON cell
            # it can only read by re-fetching with .item() (#1976).
            body = _strip_ansi(only)
        else:
            body = _nuon_table(list(head.columns), head.to_dicts())
        more = f"\n... ({rows - _DF_LLM_ROWS} more rows)" if rows > _DF_LLM_ROWS else ""
        # A frame flagging an incomplete scan (fsearch's PartialFrame: `truncated`
        # + `reason`, duck-typed so the runtime stays decoupled) must SAY so in
        # the model text: this NUON render is what the agent reads, and its repr
        # banner never reaches this path, so without the note a timed-out search
        # would read as a complete result.
        if getattr(df, "truncated", False):
            reason = getattr(df, "reason", "") or "scan incomplete"
            return f"[partial results: {reason}]\nshape: ({rows}, {cols}) | {schema}\n{body}{more}"
        return f"shape: ({rows}, {cols}) | {schema}\n{body}{more}"
    except Exception:
        # An exotic frame that resists row iteration falls back to safe NUON text.
        return _nuon(_safe_repr(df))


def _png_bytes(img: Any) -> bytes:
    """Encode a Pillow image as an optimized PNG (lossless)."""
    import io

    if img.mode not in ("RGB", "RGBA", "L"):
        img = img.convert("RGBA")
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def _jpeg_bytes(img: Any, quality: int) -> bytes:
    """Encode a Pillow image as JPEG at ``quality``, flattening any alpha onto a
    white background (JPEG has no alpha) so transparency does not turn black."""
    import io

    from PIL import Image

    if img.mode != "RGB":
        rgba = img.convert("RGBA")
        flat = Image.new("RGB", rgba.size, (255, 255, 255))
        flat.paste(rgba, mask=rgba.split()[-1])
        img = flat
    buf = io.BytesIO()
    img.save(buf, format="JPEG", quality=quality, optimize=True)
    return buf.getvalue()


def _fit_image_bytes(raw: bytes, mime: str) -> tuple[bytes, str]:
    """Bound a raster image for the model. First downscale so its longest edge is
    at most ``_IMAGE_MAX_DIM`` (aspect preserved); then ensure the encoding is at
    most ``_IMAGE_MAX_BYTES`` -- preferring a lossless PNG, falling back to JPEG
    at descending quality and, if still over, repeated downscales -- so a detailed
    screenshot can never flood the reply with megabytes of base64. Returns the
    bytes unchanged when both caps are disabled, Pillow is unavailable, or the
    image already fits untouched (a crisp PNG is kept for small UI/diagrams).
    Never raises: on any failure the original bytes/mime are returned, so fitting
    can only ever shrink the reply, never break it."""
    if _IMAGE_MAX_DIM <= 0 and _IMAGE_MAX_BYTES <= 0:
        return raw, mime
    try:
        import io

        from PIL import Image

        img = Image.open(io.BytesIO(raw))
        img.load()
        width, height = img.size
        longest = max(width, height)
        resized = False
        if _IMAGE_MAX_DIM > 0 and longest > _IMAGE_MAX_DIM:
            scale = _IMAGE_MAX_DIM / longest
            img = img.resize((max(1, round(width * scale)), max(1, round(height * scale))))
            resized = True
        # Untouched and already under the byte cap: keep the original bytes -- a
        # crisp lossless PNG is worth more than a needless re-encode.
        within_bytes = _IMAGE_MAX_BYTES <= 0 or len(raw) <= _IMAGE_MAX_BYTES
        if not resized and within_bytes:
            return raw, mime
        # Prefer a lossless PNG if it fits the byte cap.
        png = _png_bytes(img)
        if _IMAGE_MAX_BYTES <= 0 or len(png) <= _IMAGE_MAX_BYTES:
            return png, "image/png"
        # PNG is too big (a photographic / busy screenshot): switch to JPEG and
        # walk quality, then dimensions, down until it fits the byte cap.
        for quality in (85, 70, 55, 40, 25):
            jpg = _jpeg_bytes(img, quality)
            if len(jpg) <= _IMAGE_MAX_BYTES:
                return jpg, "image/jpeg"
        for _ in range(8):
            w, h = img.size
            if w <= 16 or h <= 16:
                break
            img = img.resize((max(1, w * 3 // 4), max(1, h * 3 // 4)))
            jpg = _jpeg_bytes(img, 40)
            if len(jpg) <= _IMAGE_MAX_BYTES:
                return jpg, "image/jpeg"
        return jpg, "image/jpeg"  # best effort: the smallest we could produce
    except Exception:
        return raw, mime


def _encode_image_bytes(raw: bytes, mime: str) -> dict:
    """One image as a size-bounded ``{"mime", "data"}`` base64 dict (see
    :func:`_fit_image_bytes`)."""
    raw, mime = _fit_image_bytes(raw, mime)
    return {"mime": mime, "data": base64.b64encode(raw).decode("ascii")}


def _encode_image_b64(b64: str, mime: str) -> dict:
    """Like :func:`_encode_image_bytes` for already-base64 input: decode so it can
    be downscaled, falling back to the original string if it is not valid base64
    (then it is passed through untouched)."""
    try:
        raw = base64.b64decode(b64, validate=True)
    except (ValueError, binascii.Error):
        return {"mime": mime, "data": b64}
    return _encode_image_bytes(raw, mime)


def _image_bytes_mime(value: Any) -> str | None:
    """The image mime of raw ``bytes``/``bytearray`` by magic number (PNG or
    JPEG), else None. Lets a bare ``await page.screenshot()`` -- which returns raw
    image bytes -- auto-render as an image instead of dumping a ~50k-char repr."""
    if isinstance(value, (bytes, bytearray)):
        head = bytes(value[:8])
        if head.startswith(b"\x89PNG\r\n\x1a\n"):
            return "image/png"
        if head[:3] == b"\xff\xd8\xff":
            return "image/jpeg"
    return None


def _coerce_image(value: Any) -> dict | None:
    """Coerce one ``Result.llm_images`` item to a downscaled ``{"mime", "data"}``
    (base64), or None if it is not an image we can encode. Accepts raw PNG/JPEG
    bytes, a base64 / data-URI string, a path to an image file, a matplotlib
    Figure, or any object with ``_repr_png_`` / ``_repr_jpeg_`` (a PIL image, a
    plot). Every path runs through :func:`_fit_image_bytes` (dimension and byte
    caps)."""
    if value is None:
        return None
    # Raw bytes: sniff PNG vs JPEG by magic, default to PNG.
    if isinstance(value, (bytes, bytearray)):
        raw = bytes(value)
        mime = "image/jpeg" if raw[:3] == b"\xff\xd8\xff" else "image/png"
        return _encode_image_bytes(raw, mime)
    if isinstance(value, str):
        s = value.strip()
        if s.startswith("data:image/"):
            head, _, payload = s.partition(",")
            mime = head[5:].split(";", 1)[0] or "image/png"
            return _encode_image_b64(payload, mime)
        # A filesystem path to an image.
        if len(s) < 4096 and pathlib.Path(s).is_file():
            try:
                raw = pathlib.Path(s).read_bytes()
            except OSError:
                return None
            mime = "image/jpeg" if s.lower().endswith((".jpg", ".jpeg")) else "image/png"
            return _encode_image_bytes(raw, mime)
        # Otherwise assume it is already base64-encoded PNG.
        return _encode_image_b64(s, "image/png")
    # matplotlib Figure: render to PNG.
    if type(value).__module__.startswith("matplotlib") and hasattr(value, "savefig"):
        try:
            return _encode_image_bytes(_figure_png(value), "image/png")
        except Exception:
            return None
    # Anything with a rich image repr (a PIL image, a plotly/altair object).
    for method, mime in (("_repr_png_", "image/png"), ("_repr_jpeg_", "image/jpeg")):
        repr_fn = getattr(value, method, None)
        if callable(repr_fn):
            try:
                out = repr_fn()
            except Exception:  # noqa: S112 -- intentional: skip methods that fail; try the next repr
                continue
            if out is None:
                continue
            if isinstance(out, (bytes, bytearray)):
                return _encode_image_bytes(bytes(out), mime)
            if isinstance(out, str):
                return _encode_image_b64(out, mime)
    return None


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
    # Carry the model-facing view (IX_LLM_MIME: the exact llm_result text plus the
    # downscaled llm_images) so the dashboard's raw-LLM toggle can show precisely
    # what the agent received, not just the human HTML. Stored JSON-encoded (the
    # dashboard's data map is string-valued); each image is already size-bounded,
    # so cap the whole only as a guard, dropping images (never the text) if huge.
    llm = data.get(IX_LLM_MIME)
    if isinstance(llm, dict):
        # Clip the text to the same cap as every other text mime so a huge
        # llm_result can never bypass it into SQLite or each dashboard poll, and
        # the raw view matches the model's own clipped text. Drop the images
        # (never the text) if the whole still exceeds the image cap.
        text = llm.get("text", "")
        if len(text) > _MAX_TEXT_BUNDLE:
            text = text[:_MAX_TEXT_BUNDLE] + "\n... [truncated]"
        encoded = json.dumps({"text": text, "images": llm.get("images", [])})
        if len(encoded) > _MAX_IMAGE_BUNDLE:
            encoded = json.dumps({"text": text, "images": []})
        out[IX_LLM_MIME] = encoded
    # Carry the structured human view (IX_VIEW_MIME) so pane_bridge can publish
    # it as a native `data` pane. JSON cannot be clipped without corrupting it,
    # so an oversize spec is dropped whole and the text/html fallback renders
    # instead; producers keep their payloads under the cap (see _READ_CONTEXT_MAX).
    view = data.get(IX_VIEW_MIME)
    if isinstance(view, dict):
        encoded = json.dumps(view)
        if len(encoded) <= _MAX_TEXT_BUNDLE:
            out[IX_VIEW_MIME] = encoded
    return {"data": out, "metadata": metadata or {}}


def _result_bundle(value: Any) -> dict | None:
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


def _job_outputs(job: Job) -> list[dict]:
    """A job's rich outputs for the store: every display() bundle captured while it
    ran, plus the trailing-expression result rendered the same way."""
    outs = list(job._displays)
    if not job.running() and job._result is not None:
        bundle = _result_bundle(job._result)
        if bundle is not None:
            outs.append(bundle)
    return outs


_tui_mod = None
_tui_probed = False
_vmkit_mod = None
_vmkit_probed = False


def _tui_module() -> Any:
    """The ``tui`` module if importable, cached. None when it is not available."""
    global _tui_mod, _tui_probed
    if not _tui_probed:
        _tui_probed = True
        try:
            import tui as _m

            _tui_mod = _m
        except Exception:
            # No tui in this kernel: the POC provider simply contributes nothing.
            _tui_mod = None
    return _tui_mod


def _tui_renderer(term: Any) -> Any:
    async def render() -> str:
        snap = await term.snapshot()
        return snap.to_html()

    return render


def _discover_tui_resources() -> None:
    """POC resource provider: surface every live ``Tui`` as a resource.

    Decoupled from tui-py: poll the public ``Tui.list_all()`` and register any
    terminal not seen yet (keyed by its id). When a terminal exits it drops out
    of ``list_all`` and its ``is_alive`` flips false, so the sweep closes the
    resource. This is the proof of concept; any object can be a resource via
    :func:`register_resource`.
    """
    tui = _tui_module()
    if tui is None:
        return
    try:
        live = tui.Tui.list_all()
    except Exception:
        return
    for term in live:
        rid = f"tui:{term.id}"
        if rid in resources:
            continue
        register_resource(
            term,
            id=rid,
            kind="tui",
            title=f"tui \u00b7 {term.command}",
            render=_tui_renderer(term),
            alive=lambda t=term: t.is_alive,
        )


def _vmkit_module() -> Any:
    """The ``vmkit`` module if importable, cached. None when unavailable.

    vmkit is darwin-only in the interpreter, so on other platforms this provider
    simply contributes nothing (same graceful-absence pattern as ``tui``).
    """
    global _vmkit_mod, _vmkit_probed
    if not _vmkit_probed:
        _vmkit_probed = True
        try:
            import vmkit as _m

            _vmkit_mod = _m
        except Exception:
            _vmkit_mod = None
    return _vmkit_mod


def _vmkit_renderer(driver: Any) -> Any:
    async def render() -> str:
        # The capture is a blocking pipe round trip; keep it off the event loop.
        return await asyncio.to_thread(driver.resource_html)

    return render


def _discover_vmkit_resources() -> None:
    """Resource provider: surface every booted ``vmkit.Driver`` as a resource.

    Decoupled from vmkit: poll the public ``Driver.list_all()`` and register any
    guest not seen yet (keyed by its id). When a guest stops it drops out of
    ``list_all`` and its ``is_alive`` flips false, so the sweep closes the
    resource. The live HTML is the guest's framebuffer as an inline PNG.
    """
    vmkit = _vmkit_module()
    if vmkit is None:
        return
    try:
        live = vmkit.Driver.list_all()
    except Exception:
        return
    for driver in live:
        rid = f"vm:{driver.id}"
        if rid in resources:
            continue
        register_resource(
            driver,
            id=rid,
            kind="vm",
            title=driver.title,
            render=_vmkit_renderer(driver),
            alive=lambda d=driver: d.is_alive,
        )


def _escape_html(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


# Bundled modules an agent should be able to discover without grepping source.
# The discoverable surface is declared once in `registry`; both these catalog
# lists and the startup pre-import below derive from it, so adding a module there
# is the only edit needed (see registry.py).
_API_MODULES = registry.module_names()
# Always-present namespace builtins (no import needed); see install().
_API_BUILTINS = registry.builtin_names()
_BUILTIN_TAGLINES = {b.name: b.tagline for b in registry.BUILTINS}


def _api_rows() -> list[dict]:
    """One row per discoverable helper: kernel builtins plus each bundled
    module's public surface, with signature and a one-line summary."""
    rows: list[dict] = []

    def add(where: str, name: str, obj: Any, summary: str | None = None) -> None:
        if inspect.iscoroutinefunction(obj):
            kind = "async"
        elif inspect.isclass(obj):
            kind = "class"
        elif callable(obj):
            kind = "func"
        else:
            kind = "value"
        sig = name
        if callable(obj) and not inspect.isclass(obj):
            try:
                sig = f"{name}{inspect.signature(obj)}"
            except (ValueError, TypeError):
                sig = f"{name}(...)"
        if summary is None:
            # inspect.getdoc on a plain value returns its TYPE's doc (e.g. a str
            # value yields `str(object='') -> str`), which says nothing about the
            # value -- so only fall back to getdoc for things that document
            # themselves (callables/classes/modules), never a bare value.
            doc = (inspect.getdoc(obj) or "") if callable(obj) or inspect.ismodule(obj) else ""
            summary = doc.strip().split("\n", 1)[0]
        rows.append({"where": where, "name": name, "kind": kind, "sig": sig, "summary": summary})

    target = _user_ns if _user_ns is not None else globals()
    for name in _API_BUILTINS:
        if name in target:
            # Builtins carry an authored one-line tagline in the registry; it is
            # the curated summary, so prefer it over introspection.
            add("kernel", name, target[name], summary=_BUILTIN_TAGLINES.get(name))

    for mod_name in _API_MODULES:
        try:
            mod = __import__(mod_name)
        except Exception:  # noqa: S112 -- intentional: absent module drops from catalog; discovery must not raise
            # A module that is absent or fails to import just drops out of the
            # catalog; discovery must never raise.
            continue
        names = getattr(mod, "__all__", None) or [n for n in dir(mod) if not n.startswith("_")]
        for name in names:
            obj = getattr(mod, name, None)
            if obj is not None:
                add(mod_name, name, obj)

    # Bundled third-party libraries (numpy, polars, httpx, playwright, ...): they
    # have no first-party surface to introspect, but they ARE import-ready with no
    # install step, so list them here too -- otherwise an agent treating api() as
    # the source of truth concludes they are absent (the exact trap that made a
    # bundled `playwright` look like it needed a `pip install`). Always emit a row
    # for each declared library; enrich with its version/summary when importable.
    for lib_name in (lib.name for lib in registry.LIBRARIES):
        sig = lib_name
        summary = "bundled library -- import and use it directly (help() / its own docs)"
        with contextlib.suppress(Exception):  # best-effort: absent library stays in catalog, just without version
            mod = __import__(lib_name)
            version = getattr(mod, "__version__", "")
            if version:
                sig = f"{lib_name} {version}"
            doc_text = (inspect.getdoc(mod) or "").strip().split("\n", 1)[0]
            if doc_text:
                summary = doc_text
        rows.append({"where": "library", "name": lib_name, "kind": "library", "sig": sig, "summary": summary})
    return rows


def doc(obj: Any) -> Result:
    """The signature and docstring of any object, RETURNED (not printed) as a
    Result -- so the documented "everything through Result" path also works for
    reading docs. ``help()`` only writes to stdout (not your channel) and returns
    ``None``, so ``Result(help(x))`` shows nothing; use ``doc(grep)`` instead.
    Pair it with `api()`: `api('grep')` to find a name, `doc(grep)` to read it."""
    name = getattr(obj, "__name__", None) or type(obj).__name__
    sig = ""
    if callable(obj):
        try:
            sig = f"{name}{inspect.signature(obj)}"
        except (ValueError, TypeError):
            sig = name if inspect.isclass(obj) else f"{name}(...)"
    body = inspect.getdoc(obj) or "(no docstring)"
    return Result.text(f"{sig}\n\n{body}" if sig else body)


def _build_row() -> dict:
    """The catalog's header row: which build this kernel IS. The in-band
    staleness signal (index#2110): when an agent's docs promise a helper or
    kwarg this catalog lacks, the stamp attributes the gap to a stale deploy
    instead of a phantom API. Prepended after filtering, so even a filtered
    miss (`api('check')` finding nothing) still shows it."""
    return {
        "where": "kernel",
        "name": "build",
        "kind": "build",
        "sig": f"ix-mcp {build_stamp()}",
        "summary": "this kernel's build rev and commit age; a documented helper or "
        "kwarg missing below means the running deploy predates it -- redeploy ix-mcp",
    }


def api(filter: str | None = None) -> Any:
    """A live catalog of every helper the kernel gives you: the always-present
    namespace builtins (`Result`, `cells`, `jobs`, `sh`, ...) and the public
    surface of each bundled module (`view`, `nix`, `fleet`, ...), each with
    its signature and a one-line summary. Call `api()` to discover what exists
    instead of guessing names or grepping source; pass `filter` to match a
    substring against the name, summary, or module. The first row is always the
    kernel's own build stamp (rev, commit date, age), so a catalog that lacks
    something your docs describe is attributable to a stale deploy.

    Returns a polars DataFrame (filter/sort it further, e.g.
    `api().filter(pl.col("where") == "view")`), or plain text if polars is absent.
    """
    rows = _api_rows()
    if filter:
        q = filter.lower()
        rows = [
            r for r in rows
            if q in r["name"].lower() or q in r["summary"].lower() or q in r["where"].lower()
        ]
    rows.insert(0, _build_row())
    try:
        import polars as _pl

        return _pl.DataFrame(
            rows,
            schema={"where": _pl.Utf8, "name": _pl.Utf8, "kind": _pl.Utf8, "sig": _pl.Utf8, "summary": _pl.Utf8},
        )
    except Exception:
        width = max((len(r["sig"]) for r in rows), default=0)
        return "\n".join(f'{r["where"]:>6}  {r["sig"]:<{width}}  {r["summary"]}' for r in rows)


def read_stats() -> dict[str, int]:
    """This session's cumulative file-read counters: ``total_reads`` and
    ``redundant_reads`` (a redundant read is the same file with byte-identical
    content read earlier in this session -- with perfect memory you would not
    have needed it again). Use it to check your own redundancy rate; the KPI is
    ``redundant_reads / total_reads < 1%`` (indexable-inc/ix#6440). The same
    counters are emitted to the service journal as ``mcp_read_stats`` lines."""
    job = _ix_current.get()
    session = job.session if job is not None else None
    return readstats.tracker().snapshot(session)


_TYPEERROR_CALL_RE = re.compile(
    r"^(\w+)\(\) (got an unexpected keyword argument|missing \d+ required (keyword-only argument|positional argument))"
)


def _type_error_hint(exc: TypeError) -> str:
    """Return a one-line signature hint when *exc* is a call-binding TypeError.

    Matches errors like:
      - ``grep() got an unexpected keyword argument 'max_results'``
      - ``grep() missing 1 required keyword-only argument: 'mode'``

    Looks the callable up in the user namespace (and a set of well-known module
    prefixes like ``view.``) so the hint shows the live signature. Returns an
    empty string on any failure so callers can unconditionally append it.
    """
    try:
        msg = str(exc)
        m = _TYPEERROR_CALL_RE.match(msg)
        if m is None and msg.startswith("sh() takes 1 positional argument"):
            return (
                "\nHint: sh takes one command argument. Use sh(['git', 'status']) for "
                "argv-list execution with no shell parsing, or sh('git status') when "
                "shell parsing is intended; pass cwd= instead of cd."
            )
        if not m:
            return ""
        func_name = m.group(1)
        # Resolve the callable: check user namespace and well-known module attrs.
        ns = _user_ns if _user_ns is not None else globals()
        obj = ns.get(func_name)
        if obj is None:
            # Try module-qualified names (e.g. the error says "grep" but the
            # callable lives as view.grep in the namespace).
            for mod_name in ("view", "nix", "fleet"):
                mod = ns.get(mod_name)
                if mod is not None:
                    candidate = getattr(mod, func_name, None)
                    if candidate is not None:
                        obj = candidate
                        func_name = f"{mod_name}.{func_name}"
                        break
        if obj is None or not callable(obj):
            return ""
        sig = inspect.signature(obj)
        hint = f"\nHint: the signature is {func_name}{sig}; see doc({func_name})."
        if _is_kernel_surface(func_name, obj):
            # The exact confusion of index#2110: a binding error against a
            # bundled helper reads identically whether the kwarg never existed
            # or the running kernel predates it. Only OUR surface can be stale
            # relative to an agent's docs, so user-defined callables get no
            # stamp.
            hint += (
                f" Kernel build: {build_stamp()}; if your docs describe a newer"
                f" {func_name}(), this deploy predates them -- redeploy ix-mcp."
            )
        return hint
    except Exception:
        return ""


def _is_kernel_surface(func_name: str, obj: Any) -> bool:
    """Whether a callable is part of the kernel's own catalog surface (a
    namespace builtin like ``grep``, a bundled module like ``nu``, or anything
    defined in this package), as opposed to something the user defined in a
    cell. Checks the resolved name's first segment against the registry and the
    object's defining module root, either signal suffices."""
    first = func_name.split(".", 1)[0]
    if first in _API_BUILTINS or first in _API_MODULES:
        return True
    mod = getattr(obj, "__module__", None) or getattr(type(obj), "__module__", None) or ""
    root = mod.split(".", 1)[0]
    return root == "ix_notebook_mcp" or root in _API_MODULES


async def _sweep_resources() -> None:
    """Render every live resource to the store; close the ones whose source died."""
    if _store is None or _store_conn is None:
        return
    _discover_tui_resources()
    _discover_vmkit_resources()
    now = time.time()
    for res in list(resources.values()):
        if not res.alive():
            # close() also tears down an interactive resource's action channel
            # and dispatcher, so a dead pane cannot keep accepting ix.act posts.
            res.close()
            try:
                # Very short-lived resources can open and close between flush
                # ticks. Render one terminal snapshot before closing so they
                # still appear under the job that created them.
                if res.kind == "data":
                    spec = await asyncio.wait_for(res.render_view(), timeout=2.0)
                    res.html = json.dumps(spec, default=_safe_repr)
                else:
                    res.html = await asyncio.wait_for(res.render_html(), timeout=2.0)
                res.error = None
            except Exception as exc:
                res.error = "".join(traceback.format_exception_only(type(exc), exc)).strip()
                res.html = (
                    '<pre style="color:#f7768e;margin:0">resource render failed:\n'
                    + _escape_html(res.error)
                    + "</pre>"
                )
            with contextlib.suppress(Exception):  # best-effort: store write must not kill the loop
                _store.upsert_resource(
                    _store_conn,
                    id=res.id,
                    title=res.title,
                    kind=res.kind,
                    html=res.html,
                    status="closed",
                    created_at=res.created,
                    updated_at=now,
                    execution_id=res.execution_id,
                )
                _store.close_resource(_store_conn, id=res.id, updated_at=now)
            resources.pop(res.id, None)
            continue
        status = "live"
        try:
            # Bound each render so one wedged source cannot stall the whole loop.
            # A `data` resource renders a structured {renderer, data} spec, stored
            # as JSON in the same `html` column (the pane bridge decodes it into a
            # native `data` pane); an html resource renders markup as before.
            if res.kind == "data":
                spec = await asyncio.wait_for(res.render_view(), timeout=2.0)
                res.html = json.dumps(spec, default=_safe_repr)
            else:
                res.html = await asyncio.wait_for(res.render_html(), timeout=2.0)
            res.error = None
        except Exception as exc:
            status = "error"
            res.error = "".join(traceback.format_exception_only(type(exc), exc)).strip()
            res.html = (
                '<pre style="color:#f7768e;margin:0">resource render failed:\n'
                + _escape_html(res.error)
                + "</pre>"
            )
        with contextlib.suppress(Exception):  # best-effort: store write must not kill the loop
            _store.upsert_resource(
                _store_conn,
                id=res.id,
                title=res.title,
                kind=res.kind,
                html=res.html,
                status=status,
                created_at=res.created,
                updated_at=now,
                execution_id=res.execution_id,
            )


def _drain_inputs() -> None:
    """Deliver every queued user submission to its live channel. The browser-side
    `/api/input` appended them (a separate process); this is the kernel end of
    that path. A submission for a channel that is gone (closed, or the kernel
    restarted) has no awaiter, so it is dropped -- either way the row is consumed
    so the queue stays empty between ticks.

    Dequeue (DELETE) BEFORE delivering, so a delivered submission can never be
    re-read and delivered twice (which would duplicate into a streaming channel's
    queue). If the delete fails the rows survive and retry next tick, but nothing
    was delivered yet, so there is still no duplicate. A crash between the delete
    and the put loses at most the in-flight batch, which a human just re-submits."""
    if _store is None or _store_conn is None:
        return
    try:
        pending = _store.pending_inputs(_store_conn)
    except Exception:
        return  # best-effort: a read error this tick just retries next tick
    if not pending:
        return
    try:
        _store.delete_inputs(_store_conn, [row["seq"] for row in pending])
    except Exception:
        return  # leave the rows queued; nothing delivered yet, so no duplicate
    for row in pending:
        channel = input_channels.get(row["channel"])
        if channel is not None and not channel.closed():
            try:
                payload = json.loads(row["payload"])
            except (ValueError, TypeError):
                payload = row["payload"]
            channel._deliver(payload)


# When the flusher last emitted the redundant-read stats, so it fires on the
# readstats.EMIT_WINDOW_S cadence rather than every flusher tick.
_last_readstats_emit = 0.0


async def _flusher() -> None:
    """Throttled background loop: persist every running job's output tail and
    re-render every live resource to the store so the dashboard shows both live.
    One loop for all jobs and resources (cheap)."""
    if _store is None or _store_conn is None:
        return
    global _last_readstats_emit
    _last_readstats_emit = time.time()
    while True:
        await asyncio.sleep(0.5)
        for job in list(jobs.values()):
            if job.running():
                job.line = _current_line(job)
                with contextlib.suppress(Exception):  # best-effort: store write must not kill the loop
                    _store.update_output(
                        _store_conn, job.id, job.output, job._displays or None, line=job.line
                    )
        await _sweep_resources()
        _drain_inputs()
        cells._sync()
        session._sync()
        now = time.time()
        if now - _last_readstats_emit >= readstats.EMIT_WINDOW_S:
            _last_readstats_emit = now
            with contextlib.suppress(Exception):  # best-effort: a stats emit must not kill the loop
                readstats.tracker().emit_changed()
        if _SESSION and _snapshot_dirty and not _snapshot_busy and not _restoring:
            # Fire-and-forget so a multi-second dump of a big namespace never
            # stalls the live-output mirroring this loop exists for.
            asyncio.ensure_future(_snapshot_tick())  # noqa: RUF006 -- fire-and-forget background task; loop owns the lifecycle


# --------------------------------------------------------------------------- #
# Session persistence: make the store file a reopenable notebook.
#
# With IX_MCP_SESSION=1 (set by `serve --session FILE`) the runtime checkpoints
# the user namespace into the store after cells finish, and `__ix_restore`
# (sent by the server when it reopens an existing file) loads the latest
# checkpoint back -- instant state -- then re-runs only the successful cells
# that finished after it. The failure mode is self-healing by construction: a
# checkpoint that fails to save simply leaves the previous one in place, and
# replay covers everything since it, so a reopen can be slower but never wrong.
#
# What a checkpoint holds: every name the USER bound (anything added to the
# namespace after install()), serialized per-name with dill so functions and
# classes defined in cells survive. Modules, underscore names, and values dill
# cannot serialize (sockets, running jobs, live handles) are skipped and
# reported -- no serializer can resurrect a live socket; the cell that made it
# is in the log and replays or re-runs on demand.
# --------------------------------------------------------------------------- #

_SESSION = bool(os.environ.get("IX_MCP_SESSION"))
_snapshot_dirty = False
_snapshot_busy = False
_snapshot_last = 0.0
_baseline_names: frozenset[str] = frozenset()
# The bundled modules bound behind a lazy proxy (see _LazyModule). Seeded into
# every per-session namespace (so `maps.nearby(...)` works with no import there
# too) but deliberately kept OUT of `_baseline_names`, so a user variable that
# shadows one of these names (e.g. a temp `x`) is still real user state for the
# checkpoint and the namespace pane -- an untouched proxy is dropped by TYPE, not
# by name (see _snapshot_candidates / _namespace_candidates).
_lazy_module_names: frozenset[str] = frozenset()
# True while __ix_restore is replaying. The debounced checkpoint must not fire
# then: replayed cells' source rows carry ended_at from the PREVIOUS run, so a
# mid-restore checkpoint would advance the anchor past the cells not yet
# replayed -- a crash right after it would lose them. The restore takes one
# explicit checkpoint when it completes instead.
_restoring = False

# At most one checkpoint per this many seconds: a burst of short cells costs one
# dump, not one per cell.
_SNAPSHOT_MIN_INTERVAL = 5.0

# Per-value ceiling on a serialized binding. A frame this large makes every
# checkpoint write (and the session file) balloon; past it the value is skipped
# and the cell that built it replays on reopen instead.
_SNAPSHOT_MAX_VALUE_BYTES = 64_000_000


def _mark_snapshot_dirty() -> None:
    global _snapshot_dirty
    if _SESSION:
        _snapshot_dirty = True


def _dill() -> Any:
    """The serializer for checkpoints: dill (handles functions/classes defined
    in cells, the common case for an agent session), else stdlib pickle so a
    bare interpreter without dill still checkpoints plain data."""
    try:
        import dill

        return dill
    except Exception:
        import pickle

        return pickle


def _snapshot_payload(candidates: dict) -> tuple[bytes, list[str], list[dict]]:
    """Serialize ``candidates`` per-name (one unpicklable value must not void the
    whole checkpoint). Returns (blob, kept names, skipped). Runs off the loop --
    dumping a big namespace is CPU-bound."""
    import pickle

    dumper = _dill()
    # recurse=True makes dill pickle only the globals a function actually
    # references, instead of its entire ``__globals__`` (the whole user
    # namespace, which drags every unpicklable live object into every helper).
    # The restored function gets its own copy of those referenced globals; a
    # helper that mutates module-level state through them is the one shape this
    # cannot preserve, and the cell that defined it is in the log to re-run.
    kwargs = {"recurse": True} if getattr(dumper, "__name__", "") == "dill" else {}
    named: dict[str, bytes] = {}
    skipped: list[dict] = []
    for name, value in candidates.items():
        try:
            payload = dumper.dumps(value, **kwargs)
        except Exception as exc:
            skipped.append({"name": name, "reason": f"{type(exc).__name__}: {exc}"[:200]})
            continue
        if len(payload) > _SNAPSHOT_MAX_VALUE_BYTES:
            skipped.append({"name": name, "reason": f"too large ({len(payload)} bytes)"})
            continue
        named[name] = payload
    # The outer envelope is stdlib pickle (a dict of str -> bytes always
    # pickles), so restore can open it even when dill versions drift; each inner
    # value is tried independently there too.
    return pickle.dumps(named), sorted(named), skipped


# IPython's own underscore bindings, created lazily as cells run (so they are
# NOT in the baseline): the result caches (`_`, `__`, `___`, `_1`, ...), the
# input caches (`_i`, `_ii`, `_iii`, `_i1`, ...), and the history/state dicts.
# These are kernel machinery, not user state; everything else a user binds --
# including a single-underscore name like `_cfg` -- is real state and must be
# checkpointed, not silently dropped.
_IPYTHON_MACHINERY = re.compile(r"_+|_i+|_i\d+|_\d+|_oh|_dh|_ih|_exit_code")


def _snapshot_candidates(ns: dict) -> dict:
    """The names a checkpoint covers: bound after install() (so the runtime's own
    surface and the preamble never bloat the file), not dunders or IPython's
    history machinery, not modules (an import is one cheap replayed line; module
    objects pickle poorly)."""
    return {
        name: value
        for name, value in ns.items()
        if name not in _baseline_names
        and not name.startswith("__")
        and not _IPYTHON_MACHINERY.fullmatch(name)
        and not isinstance(value, types.ModuleType)
        and not isinstance(value, _LazyModule)  # untouched lazy proxy: not user state
    }


def _namespace_candidates(ns: dict) -> dict:
    """The user-bound names the dashboard's namespace pane shows: like
    :func:`_snapshot_candidates` (drop baseline helpers, dunders, IPython history
    machinery) but keep modules — an imported ``pl`` is worth seeing in the
    namespace even though it is not checkpointed."""
    return {
        name: value
        for name, value in ns.items()
        if name not in _baseline_names
        and not name.startswith("__")
        and not _IPYTHON_MACHINERY.fullmatch(name)
        and not isinstance(value, _LazyModule)  # untouched lazy proxy: not user state
    }


def _store_file() -> str | None:
    """The on-disk path behind ``_store_conn`` (PRAGMA database_list), so a
    worker thread can open its own connection to the same file. None for a
    non-file store (in-memory test connections)."""
    if _store_conn is None:
        return None
    try:
        row = _store_conn.execute("PRAGMA database_list").fetchone()
        return row[2] or None
    except Exception:
        return None


async def _snapshot_now() -> dict:
    """Take one checkpoint now. The timestamp is taken BEFORE the namespace is
    copied: a cell finishing mid-dump may or may not be captured, and an earlier
    stamp errs toward replaying it -- re-running a captured cell overwrites equal
    state, while the reverse (assuming an uncaptured cell was captured) would
    lose it.

    Both the serialization AND the SQLite write run in the worker thread: a
    multi-hundred-MB blob INSERT on the event loop stalls every queued cell and
    the live-output mirroring for the write's duration. ``_store_conn`` is bound
    to the loop thread (check_same_thread), so the thread opens its own
    connection to the same file; WAL + busy_timeout serialize it against the
    kernel's writer."""
    if _store is None or _store_conn is None:
        return {"names": [], "skipped": []}
    ns = _user_ns if _user_ns is not None else globals()
    created = time.time()
    candidates = _snapshot_candidates(dict(ns))
    path = _store_file()

    def _dump_and_save() -> tuple[list[str], list[dict]]:
        blob, names, skipped = _snapshot_payload(candidates)
        conn = _store.connect(path)
        try:
            _store.save_snapshot(conn, created_at=created, blob=blob, names=names, skipped=skipped)
        finally:
            conn.close()
        return names, skipped

    if path is not None:
        names, skipped = await asyncio.to_thread(_dump_and_save)
    else:
        # A non-file store (in-memory test connection): the write must use the
        # loop's own connection, and such a store is small by construction.
        blob, names, skipped = await asyncio.to_thread(_snapshot_payload, candidates)
        _store.save_snapshot(
            _store_conn, created_at=created, blob=blob, names=names, skipped=skipped
        )
    return {"names": names, "skipped": skipped}


async def _snapshot_tick() -> None:
    global _snapshot_busy, _snapshot_dirty, _snapshot_last
    if _snapshot_busy or time.time() - _snapshot_last < _SNAPSHOT_MIN_INTERVAL:
        return
    _snapshot_busy = True
    _snapshot_dirty = False
    try:
        await _snapshot_now()
    except Exception:  # noqa: S110 -- best-effort: previous checkpoint stays in place; finally always runs
        # Leave the previous checkpoint in place; replay covers the gap on
        # reopen. The next finished cell re-marks dirty, so this also cannot
        # spin on a persistently failing dump.
        pass
    finally:
        _snapshot_last = time.time()
        _snapshot_busy = False


async def __ix_snapshot() -> Result:
    """Checkpoint the namespace to the session store right now (the server sends
    this on shutdown; callable any time)."""
    info = await _snapshot_now()
    kept, skipped = len(info["names"]), info["skipped"]
    note = f"session checkpoint: {kept} names saved"
    if skipped:
        note += f", {len(skipped)} skipped ({', '.join(s['name'] for s in skipped[:10])})"
    return Result.ok(note)


# Ceiling on one replayed cell. Everything in the replay set completed once, so
# this only trips on a cell whose duration is environment-dependent (it waited
# on something external); such a cell is cancelled and reported, not allowed to
# wedge the reopen forever.
_REPLAY_BUDGET = 600.0


async def __ix_restore() -> None:
    """Reopen a session: load the latest checkpoint into the namespace (instant
    state), then re-run the successful cells that finished after it, oldest
    first. Prints its summary -- this runs as a raw execute outside any job, so
    the prints reach the server's log, not a job buffer."""
    global _restoring
    if _store is None or _store_conn is None:
        print("session restore: no store configured")
        return
    _restoring = True
    try:
        await _restore_body()
    finally:
        _restoring = False


async def _restore_body() -> None:
    ns = _user_ns if _user_ns is not None else globals()
    import pickle

    loader = _dill()
    snap = None
    try:
        snap = _store.latest_snapshot(_store_conn)
    except Exception as exc:
        print(f"session restore: checkpoint read failed ({exc}); replaying the full log")
    restored: list[str] = []
    load_failed: list[str] = []
    if snap is not None:
        try:
            named = pickle.loads(snap["blob"])  # noqa: S301 -- trusted data: blob is written by this process only
        except Exception as exc:
            print(f"session restore: checkpoint decode failed ({exc}); replaying the full log")
            named, snap = {}, None
        else:
            for name, payload in named.items():
                try:
                    ns[name] = loader.loads(payload)
                    restored.append(name)
                except Exception:  # per-name restore; individual failures tracked in load_failed
                    load_failed.append(name)
    since = snap["created_at"] if snap is not None else None
    rows: list[dict] = []
    try:
        rows = _store.replayable(_store_conn, since)
    except Exception as exc:
        print(f"session restore: could not read the replay set ({exc})")
    replay_failed: list[str] = []
    for row in rows:
        job = await __ix_run(
            row["code"], budget=_REPLAY_BUDGET, name=f"replay:{row['name'] or row['id']}", kind="replay"
        )
        if job.running():
            job.cancel()
            replay_failed.append(f"{row['id']} (exceeded {_REPLAY_BUDGET:.0f}s)")
        elif job.status != "done":
            replay_failed.append(row["id"])
    if _SESSION:
        # Fold the replayed state into a fresh checkpoint so the NEXT reopen is
        # all-instant (and replays never feed future replays).
        with contextlib.suppress(Exception):  # best-effort: skip checkpoint failure during restore
            await _snapshot_now()
    parts = [f"{len(restored)} names restored instantly"]
    if snap is not None and snap.get("skipped"):
        parts.append(f"{len(snap['skipped'])} not in checkpoint ({', '.join(s['name'] for s in snap['skipped'][:10])})")
    if load_failed:
        parts.append(f"{len(load_failed)} failed to load ({', '.join(load_failed[:10])})")
    parts.append(f"{len(rows)} cells replayed")
    if replay_failed:
        parts.append(f"{len(replay_failed)} replays failed ({', '.join(replay_failed[:10])})")
    print("session restore: " + "; ".join(parts))


# Per-MCP-session namespaces, keyed by the session id the server passes through
# ``__ix_exec``. One kernel serves every client of the HTTP transport, and with a
# single shared namespace parallel agents clobber each other's variables (observed
# in production). Each session therefore gets its own module-level globals dict,
# created lazily and seeded from the shared read-only area: the runtime surface
# plus the bundled helpers, i.e. exactly the names ``install()`` captured in
# ``_baseline_names``. The helper OBJECTS stay shared (jobs/cells/resources are
# one registry, so the dashboard and cross-session job paging keep working); only
# the name bindings are per-session, so one session's assignments never shadow
# another's. Within one session the dict persists across calls -- the kernel's
# persistent-namespace contract, unchanged. No session id (the stdio transport,
# replay, the in-process tests) keeps today's single shared namespace, which is
# also what session checkpoint/restore covers.
_session_namespaces: dict[str, dict] = {}


def _shared_ns() -> dict:
    return _user_ns if _user_ns is not None else globals()


def _session_ns(session: str | None) -> dict:
    """The globals dict for ``session``: the shared user namespace when no
    session id is given, else that session's own dict (created on first use)."""
    if not session:
        return _shared_ns()
    ns = _session_namespaces.get(session)
    if ns is None:
        shared = _shared_ns()
        # install() ran: seed exactly the helper surface. A bare runtime where it
        # did not (one-shot eval paths) falls back to forking the whole shared
        # namespace, so the session still sees Result and friends.
        names = _baseline_names or frozenset(shared)
        ns = {name: shared[name] for name in names if name in shared}
        # Give the session its OWN fresh lazy proxies, so `maps.nearby(...)` works
        # with no import here too -- but never copy `shared[name]` for a lazy name:
        # a no-session cell (or a restored checkpoint) may have rebound that name to
        # user state (e.g. `x = 5`), and copying it would leak one context's value
        # into every fresh session. A fresh proxy is stateless and correct.
        for name in _lazy_module_names:
            ns[name] = _LazyModule(name)
        _session_namespaces[session] = ns
    return ns


async def __ix_run(
    code: str,
    budget: float = 15.0,
    name: str | None = None,
    kind: str = "cell",
    session: str | None = None,
    topic: str | None = None,
) -> Job:
    """Run ``code`` as a task; wait up to ``budget`` for it; return the Job either
    way (done, or still running in the background). ``session`` selects the
    namespace the code runs in (see :func:`_session_ns`)."""
    ns = _session_ns(session)
    job = Job(code, name, budget=budget, kind=kind, topic=topic or globals()["session"].topic, session=session)
    job._ns = ns
    jobs[job.id] = job
    job.task = asyncio.ensure_future(_runner(job, ns))
    await asyncio.wait({job.task}, timeout=budget)
    if not job.task.done():
        job.backgrounded = True
    return job


# How many chars of a job's output/result the per-call summary carries inline.
# The full output stays in the kernel as ``jobs[id]`` (paged via tail/head/slice/
# grep/lines); the summary also reports the full sizes so the server can tell the
# caller when a reply was truncated and point at the job to page.
_SUMMARY_CHARS = 50_000


def _result_text(job: Job) -> str:
    """The job result's model-facing text (a Result's ``llm_result``, else its
    repr). Feeds ``Job.pageable`` and the per-call summary, so it must always be
    a ``str`` -- a non-str ``llm_result`` (now coerced at Result construction,
    but possibly present on an old object) falls back to the repr rather than
    crash the summary path."""
    if job._result is None:
        return ""
    text = getattr(job._result, "llm_result", None)
    if isinstance(text, str) and text:
        return text
    return _safe_repr(job._result)


def _job_summary(job: Job) -> dict:
    """The structured per-call summary the MCP server parses. ``output_chars`` and
    ``result_chars`` are the *full* sizes (the inline ``output`` is only a tail),
    so the server can detect a truncated reply and tell the caller to page
    ``jobs['<id>']``."""
    return {
        "id": job.id,
        "name": job.name,
        "topic": job.topic,
        "status": job.status,
        "running": job.running(),
        "output": job.tail(_SUMMARY_CHARS),
        "output_chars": len(job.output),
        "result": None if job._result is None else _safe_repr(job._result),
        "result_chars": len(_result_text(job)),
        "error": job.error,
        # Wall-clock seconds the run has taken (final once it ends, elapsed-so-far
        # while it is still backgrounded). Surfaced in every reply so the caller
        # notices a slow run and treats it as a problem to fix -- usually a sync
        # call freezing the loop (make it async/background), not just an FYI.
        "elapsed_s": round((job.ended or time.time()) - job.started, 2),
        # Where a still-running job is right now (cell line), so a budget-expired
        # reply can say not just "running" but "running, on line N".
        "line": _current_line(job) if job.running() else None,
    }


def history(n: int = 20) -> Result:
    """A compact, newest-last listing of the most recent runs in this kernel, so
    you can see what is available to drill into without remembering ids. Each row
    is a ``jobs['<id>']`` you can page: ``.tail()/.head()/.slice()/.lines()/
    .grep()`` for its output, ``.output`` for the full stdout, ``.result`` for the
    value. The full runs persist in the kernel; this is just the index over them."""
    items = list(jobs.values())[-n:]
    header = f"{'id':<10}{'name':<18}{'status':<10}{'dur':>8}{'out':>9}{'result':>9}"
    rows = [header]
    for job in items:
        dur = (job.ended or time.time()) - job.started
        name = "" if job.name == job.id else job.name
        rows.append(
            f"{job.id:<10}{name[:17]:<18}{job.status:<10}{dur:>7.1f}s"
            f"{len(job.output):>9}{len(_result_text(job)):>9}"
        )
    body = "\n".join(rows) if items else "(no runs yet)"
    html = f'<pre class="ix-result">{_escape_html(body)}</pre>'
    return Result.text(body, html=html)


def _emit(job: Job) -> None:
    """Publish a structured summary the MCP server parses, plus the result's rich
    repr (image/HTML/table) as normal display output the server already renders."""
    from IPython.display import display, publish_display_data

    summary = _job_summary(job)
    publish_display_data({JOB_MIME: summary, "text/plain": f"[{job.id}] {job.status}"})
    # On success job._result is always a Result (the runner enforces it); display
    # it so the server can unpack the model view and the dashboard the human one.
    if job.status == "done" and job._result is not None:
        with contextlib.suppress(Exception):  # best-effort: rich display failure must not block the summary
            display(job._result)


async def __ix_exec(
    code: str,
    budget: float = 15.0,
    name: str | None = None,
    session: str | None = None,
    topic: str | None = None,
) -> None:
    """The MCP server's per-call entrypoint: run with a budget, emit the summary.
    ``session`` is the caller's MCP session id (per-session namespace; None for
    the shared one)."""
    job = await __ix_run(code, budget=budget, name=name, session=session, topic=topic)
    _emit(job)


def __ix_emit_read_stats_final() -> None:
    """Emit every session's final ``mcp_read_stats`` line. The server calls this
    in-kernel from its shutdown ``finally`` block, BEFORE it kills the kernel with
    ``shutdown_kernel(now=True)`` (a SIGKILL, which no atexit hook survives), so
    the counts accrued since the last periodic emit are flushed to the journal."""
    readstats.tracker().emit_final()


def _existing_file(value: Any) -> pathlib.Path | None:
    """``value`` as a :class:`pathlib.Path` when it is a string naming an
    existing file, else None. The one rule `__ix_read` applies to both the raw
    ``target`` and to a string an expression evaluates to."""
    if not isinstance(value, str) or not value or len(value) > 4096 or "\n" in value:
        return None
    try:
        candidate = pathlib.Path(value).expanduser()
        return candidate if candidate.is_file() else None
    except OSError:
        return None


def _tilde(path: Any) -> str:
    """An absolute path with the home directory collapsed to ``~`` for a compact,
    privacy-friendly note (``/Users/me/.ix/trace/x`` -> ``~/.ix/trace/x``)."""
    text = str(path)
    home = str(pathlib.Path.home())
    if text == home:
        return "~"
    if text.startswith(home + os.sep):
        return "~" + text[len(home):]
    return text


# Cap on the highlight context shipped to the dashboard for one read. The
# frontend tokenizes the WHOLE file so a mid-file slice still highlights
# correctly (open strings, nested blocks); past this size only the slice
# travels, so a huge file never rides every dashboard poll. Stays under
# _MAX_TEXT_BUNDLE even after JSON escaping so _normalize_bundle never drops it.
_READ_CONTEXT_MAX = 128 * 1024

# Well-known extensionless files -> highlight grammar. Anything else hints its
# bare extension; the frontend aliases (py -> python) and falls back to plain.
_NAMED_LANGS = {"dockerfile": "docker", "makefile": "make"}


def _read_lang(path: Any) -> str | None:
    """The dashboard highlight hint for a file: a named grammar for well-known
    extensionless files, else the lowercased bare extension."""
    name = pathlib.Path(path).name.lower()
    return _NAMED_LANGS.get(name) or pathlib.Path(name).suffix.lstrip(".").lower() or None


def _clip_lines(lines: list[str], budget: int) -> list[str]:
    """The longest line-boundary prefix whose joined size fits ``budget``. When
    even the first line alone exceeds the budget (minified JSON, a giant log
    line), a character prefix of it is returned rather than nothing, so a
    clipped view is never blank."""
    out: list[str] = []
    used = 0
    for line in lines:
        used += len(line) + 1
        if used > budget:
            break
        out.append(line)
    if not out and lines:
        out.append(lines[0][:budget])
    return out


async def __ix_read(target: Any, start: int | None = None, end: int | None = None, session: str | None = None) -> Result:
    """Read a file (or evaluate a kernel value) FOR THE MODEL, quietly.

    Returns a Result whose ``llm_result`` is the full text the model receives and
    whose ``user_view`` is a structured ``file-view`` spec the dashboard renders
    natively (highlighted card with the read span), so a large read informs the
    model without flooding the dashboard. ``target`` is read as a file when it
    names an existing file, otherwise evaluated as a Python expression in the user
    namespace (e.g. ``jobs['ab12'].output``, a variable you bound). ``start`` and
    ``end`` select a 1-based inclusive line range. ``session`` evaluates the
    expression in that MCP session's namespace (the same one its ``python_exec``
    cells run in), so a variable bound there resolves. Backs the ``read`` MCP tool.
    """
    ns = _session_ns(session)
    value = None
    path = _existing_file(target)
    if path is None:
        # Not a file on disk: evaluate the expression. If the VALUE is a string
        # naming an existing file (`os.path.join(...)`, a variable holding a
        # path), the same file rule applies to it -- an expression yielding a
        # path reads the file, never echoes the path string back.
        value = eval(target, ns) if isinstance(target, str) else target  # noqa: S307 -- intentional: evaluating user-provided expression in kernel namespace
        path = _existing_file(value)
    if path is not None:
        # Off the loop: a large file read is blocking I/O, the one thing that
        # freezes every other job on the shared event loop.
        full = await asyncio.to_thread(path.read_text, errors="replace")
        label = _tilde(path)
        lang = _read_lang(path)
    else:
        full = value if isinstance(value, str) else _safe_repr(value)
        label = target if isinstance(target, str) else _safe_repr(target)
        lang = None
    # '\n' is the ONE line boundary, matching the renderer's split — str.splitlines
    # would also break on \f/\v/\x85/U+2028..., desyncing the gutter numbers and
    # span meta from the rows actually displayed. A trailing newline is a
    # terminator, not a phantom last line.
    lines = full.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    total = len(lines)
    if start is not None or end is not None:
        lo = max((start or 1) - 1, 0)
        hi = total if end is None else min(end, total)
        selected = lines[lo:hi]
        body = "\n".join(selected)
        first, last = lo + 1, lo + len(selected)
    else:
        selected = lines
        body = full
        first, last = 1, total
    if path is not None:
        # Track this read for the redundant-read KPI. Hash the payload the agent
        # actually RECEIVED (`body`, i.e. the line slice for a ranged read), not
        # the whole file -- so reading lines 1-100 then 101-200 is two novel reads,
        # not a false redundancy. Hashing is CPU-bound and `body` can be large, so
        # it runs off the event loop (like the read itself); only the fast set
        # lookup + counter update lands back on the loop. Never a second disk read.
        _digest = await asyncio.to_thread(readstats.digest, path, body)
        readstats.tracker().record_digest(session, _digest)
    # Display context: the whole file when it fits, else the slice, else a
    # line-clipped head of the slice. `start`/`end`/`total`/`chars` always
    # describe what the model received; `text`+`context_start` describe display,
    # and `truncated` marks a display copy that omits part of the read span so
    # the card never silently poses as the full range.
    truncated = False
    if len(full) <= _READ_CONTEXT_MAX:
        text, context_start = full, 1
    elif len(body) <= _READ_CONTEXT_MAX:
        text, context_start = body, first
    else:
        text, context_start = "\n".join(_clip_lines(selected, _READ_CONTEXT_MAX)), first
        truncated = True
    return Result(
        user_view={
            "renderer": "file-view",
            "data": {
                "label": label,
                "file": path is not None,
                "lang": lang,
                "text": text,
                "context_start": context_start,
                "start": first,
                "end": last,
                "total": total,
                "chars": len(body),
                "truncated": truncated,
            },
        },
        # Plain-HTML fallback for hosts (and the mixed-rich-output pane path)
        # that do not render the structured view; the display context, escaped.
        user_html=f'<pre class="ix-result">{_escape_html(text)}</pre>',
        llm_result=body,
    )



def _install_display_capture(shell: Any) -> None:
    """Route display() / rich auto-display made *inside a job* to that job's output
    list (still forwarding to IOPub for the agent's reply), so the dashboard can
    show images and HTML tables, not just text."""
    pub = shell.display_pub
    if getattr(pub, "_ix_wrapped", False):
        return
    original = pub.publish

    def publish(data: Any, metadata: Any = None, **kwargs: Any) -> Any:
        job = _ix_current.get()
        if job is not None and isinstance(data, dict) and JOB_MIME not in data:
            bundle = _normalize_bundle(data, metadata)
            if bundle["data"]:
                job._displays.append(bundle)
        return original(data, metadata, **kwargs)

    pub.publish = publish
    pub._ix_wrapped = True


def _figure_png(fig: Any) -> bytes:
    import io

    buf = io.BytesIO()
    fig.savefig(buf, format="png", bbox_inches="tight")
    return buf.getvalue()


def _register_rich_formatters(shell: Any) -> None:
    """Make the bundled view objects render richly in the dashboard.

    Two gaps in a bare ipykernel: it only wires the inline matplotlib png
    formatter after ``%matplotlib inline``, and IPython's ``text/html`` formatter
    consults only ``_repr_html_``, never the ``__html__`` protocol that htpy (the
    bundled HTML builder) and markupsafe implement. Without the latter, an htpy
    element handed to ``cells.add``/``Result.of`` falls back to its ``repr``
    (``<Element '<div ...>...'>``) instead of rendering. Register both lazily by
    type name so importing matplotlib or htpy stays the user's choice."""
    with contextlib.suppress(Exception):  # no display formatter (non-IPython host) or matplotlib too old to wire
        png = shell.display_formatter.formatters["image/png"]
        png.for_type_by_name("matplotlib.figure", "Figure", _figure_png)
    with contextlib.suppress(Exception):  # no display formatter, or htpy/markupsafe layout mismatch
        html = shell.display_formatter.formatters["text/html"]
        html.for_type_by_name("htpy._elements", "BaseElement", lambda el: el.__html__())
        html.for_type_by_name("markupsafe", "Markup", str)


_user_ns: dict | None = None


def _install_signal_handlers() -> None:
    """Wire the two operator signals the MCP server uses to inspect or rescue a
    kernel whose event loop is blocked by a synchronous call.

    SIGUSR1: faulthandler dumps every thread's Python stack to the file named by
    ``IX_MCP_KERNEL_TRACE`` (kept by ``kernel.TRACE_ENV``). The handler is C-level
    so it runs even while the main thread is parked in a blocking call; the
    ``kernel_trace`` tool reads the file back.

    SIGUSR2: raise ``KeyboardInterrupt`` in the main thread when a job is running.
    Every cell runs as an async cell (``await __ix_exec(...)``), and ipykernel
    interrupts async cells by cancelling the asyncio task, which a synchronous
    call never yields to, so SIGINT cannot break a wedged cell. A custom handler
    that raises does: the signal interrupts the blocking syscall and the handler
    runs inline at the blocked frame, where ``_runner`` catches it."""
    global _trace_file
    import faulthandler
    import signal

    trace_path = os.environ.get("IX_MCP_KERNEL_TRACE")
    if trace_path:
        _trace_file = pathlib.Path(trace_path).open("w")  # noqa: SIM115 -- trace file must outlive this scope (faulthandler writes to it for the process lifetime)
        # enable() handles fatal signals (SIGSEGV/SIGABRT) -> stderr; register()
        # adds the on-demand SIGUSR1 all-thread dump the kernel_trace tool reads.
        faulthandler.enable()
        faulthandler.register(signal.SIGUSR1, file=_trace_file, all_threads=True, chain=False)

    def _break(signum: int, frame: Any) -> None:
        # Only raise while a job is on the stack; a stray signal to an idle kernel
        # must not blow up the event loop. The handler runs in the interrupted
        # frame's context, so it sees the running job's ContextVar. Flag the job so
        # _runner can tell this watchdog interrupt from a KeyboardInterrupt the
        # user's own code raised (which must keep its real traceback).
        job = _ix_current.get()
        if job is not None:
            job.interrupted_by_watchdog = True
            raise KeyboardInterrupt("ix: cell exceeded its budget while blocking the event loop")

    with contextlib.suppress(ValueError):  # signal.signal only works on main thread; tests call install() off it
        signal.signal(signal.SIGUSR2, _break)


class _LazyModule:
    """A stand-in bound into the kernel namespace so a bundled module is usable
    with no ``import`` -- without paying its import cost until it is actually
    touched. The first *public* attribute access imports the real module (cached by
    ``sys.modules``, so repeat access is ~free) and delegates. An untouched module
    therefore costs nothing at startup -- which matters for the framework-heavy
    macOS modules (``maps`` alone pulls in MapKit + CoreLocation, ~120ms) -- and a
    platform-absent one (a macOS-only module on Linux) raises an ordinary
    ``ImportError`` only when first used, exactly as an explicit ``import`` would.
    An explicit ``import maps`` still imports the same module and rebinds the name
    to the real module object, so both styles agree.

    Access to a dunder / underscore-prefixed name does NOT import: those are
    introspection probes (pickle's ``__reduce_ex__``, IPython's ``_repr_html_``,
    ``hasattr`` checks in the namespace/snapshot machinery), and importing a
    macOS-only module on Linux just to answer one would raise spuriously. The
    public module API a caller actually wants (``maps.nearby``, ``slack.channels``)
    is always a non-underscore name, so this never blocks real use.
    """

    __slots__ = ("_ix_name",)

    def __init__(self, name: str) -> None:
        self._ix_name = name

    def __getattr__(self, attr: str) -> Any:
        # __getattr__ only fires for names absent on the class/slot, so the
        # `_ix_name` slot resolves normally and never recurses here. Refuse to
        # import for private/introspection names (see class docstring).
        if attr.startswith("_"):
            raise AttributeError(attr)
        return getattr(__import__(self._ix_name), attr)

    def __repr__(self) -> str:
        return f"<bundled module {self._ix_name!r} (lazy: imports on first use)>"


def install(user_ns: dict | None = None) -> None:
    """Wire the runtime into the kernel: tee stdout/err, open the store, start the
    flusher, install the rescue/trace signal handlers, and expose the registry +
    entrypoints in the user namespace."""
    global _store, _store_conn, _user_ns, _shell
    _user_ns = user_ns

    _install_signal_handlers()

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
    target["history"] = history
    target["doc"] = doc
    target["Job"] = Job
    target["Result"] = Result
    target["cells"] = cells
    target["Cells"] = Cells
    target["session"] = session
    # Seed the default session label with this kernel's working directory; the
    # connecting client's identity is folded in later (see Kernel.set_client).
    with contextlib.suppress(OSError):
        session._workdir = process_cwd().name or ""
        session._rev += 1  # ensure the first flush mirrors the default to the store
    target["resources"] = resources
    target["Resource"] = Resource
    target["register_resource"] = register_resource
    target["Input"] = Input
    target["ask"] = ask
    target["notify"] = notify
    target["watch_pr"] = watch_pr
    target["input_channels"] = input_channels
    target["__ix_run"] = __ix_run
    target["__ix_exec"] = __ix_exec
    target["__ix_read"] = __ix_read
    target["__ix_emit_read_stats_final"] = __ix_emit_read_stats_final
    target["__ix_snapshot"] = __ix_snapshot
    target["__ix_restore"] = __ix_restore
    target["DASHBOARD_URL"] = os.environ.get("IX_MCP_DASHBOARD_URL", "")
    # `sh`/`zsh` are RETIRED (agents shell out through `await nu(...)`; the sh
    # module's public entry points now raise a migration hint). Bind them anyway
    # so a stale `await sh(cmd)` in an old transcript fails LOUDLY with that hint
    # rather than a bare NameError. The kernel's own internals reach the private
    # runner via `from sh import _exec`, which is never bound into the namespace.
    with contextlib.suppress(Exception):  # sh may be absent outside the bundled interpreter; skip it
        import sh as _sh_module

        target["sh"] = _sh_module
        target["zsh"] = _sh_module.zsh
    # Bind the filesystem-search helpers as top-level callables (`await grep(...)`
    # / `find(...)` / `spotlight(...)`) the way `sh` is bound, so the most common
    # search/listing actions need no import. They live in the bundled `fsearch`
    # module (ripgrep/fd/Spotlight via `sh`, each returning a polars frame); an
    # explicit `import fsearch` returns the same functions.
    with contextlib.suppress(Exception):  # fsearch may be absent outside the bundled interpreter; skip it
        import fsearch as _fsearch_module

        target["grep"] = _fsearch_module.grep
        target["find"] = _fsearch_module.find
        target["spotlight"] = _fsearch_module.spotlight
    # Pre-bind the most-reached-for bundled module so `view.ls(...)` works with no
    # import, the way Result/cells/jobs/sh do (an explicit `import view` returns
    # the same object). It is already imported at startup (01-ix-polars installs
    # view's renderer), so binding it here costs nothing; heavier modules (nix,
    # fleet, search) stay import-on-demand to keep the namespace lean.
    for _mod_name in registry.preimport_names():
        with contextlib.suppress(Exception):  # best-effort per-module import; continue on missing modules
            target[_mod_name] = __import__(_mod_name)
    # The kernel is async-first and polars-first: nearly every session reaches
    # for asyncio (ensure_future / sleep), json (every CLI's --json output), and
    # pl within its first cells, and a NameError on `asyncio` in an async kernel
    # is pure friction (observed twice in one 2026-06-10 session). Bound like
    # sh/view; an explicit import returns the same module.
    target["asyncio"] = asyncio
    target["json"] = json
    with contextlib.suppress(Exception):  # polars may be absent; skip binding pl
        import polars as _polars_mod

        target["pl"] = _polars_mod
    target["api"] = api
    target["read_stats"] = read_stats
    # Failures of fire-and-forget tasks, newest last (see the deque's comment).
    # A report also lands in the spawning job's output, tagged `[task_errors]`,
    # which is how the name advertises itself.
    target["task_errors"] = task_errors

    # Everything in the namespace up to here is the runtime's own surface plus
    # the kernel preamble -- not user state. Session checkpoints cover only the
    # names bound after this line (see _snapshot_candidates).
    global _baseline_names, _lazy_module_names
    _baseline_names = frozenset(target)
    # Bind every other bundled module behind a lazy proxy, so `await maps.nearby(...)`
    # works with no `import maps` just like sh/view -- but deferring the import to
    # first use, so framework-heavy modules (maps pulls in MapKit + CoreLocation,
    # ~120ms) and platform-absent ones cost nothing at startup. Bound AFTER the
    # baseline snapshot on purpose: these names must NOT count as runtime surface,
    # so a user variable that shadows one (a temp `x`, say) stays real user state
    # for the checkpoint / namespace pane; an untouched proxy is excluded by type
    # instead (see _snapshot_candidates / _namespace_candidates). _session_ns seeds
    # them explicitly so per-session namespaces get them too.
    _lazy_names = [m for m in registry.module_names() if m not in target]
    _lazy_module_names = frozenset(_lazy_names)
    for _mod_name in _lazy_names:
        target[_mod_name] = _LazyModule(_mod_name)
    # A fresh session starts with no recorded references (the namespace is empty of
    # user names; refs accumulate as runs touch them).
    _name_refs.clear()

    with contextlib.suppress(RuntimeError):  # no event loop yet (sync context): flusher and task watch are optional
        loop = asyncio.get_event_loop()
        # Watch first, then spawn: the flusher must itself be a watched task,
        # and its module-global reference (which prevents GC, RUF006) is
        # exactly what would otherwise keep a crashed flusher silent forever.
        _install_task_failure_watch(loop)
        global _flusher_task
        _flusher_task = loop.create_task(_flusher())
