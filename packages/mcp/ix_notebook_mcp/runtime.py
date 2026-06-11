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
import dataclasses
import inspect
import json
import os
import pathlib
import re
import sys
import time
import traceback
import types
import uuid
from collections.abc import Mapping

from . import registry

_ix_current: contextvars.ContextVar = contextvars.ContextVar("ix_current_job", default=None)

# Cap on a single job's captured output kept in memory (and mirrored to the store
# and the dashboard). A chatty/runaway job keeps only the most recent slice, so
# memory, store writes, and poll payloads all stay bounded.
_MAX_OUTPUT_CHARS = 256_000

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

_RESULT_REQUIRED = (
    "python_exec: a cell must declare its result. Either END with a Result(...), "
    "or `yield Result(...)` one or more times to stream results as you go. This "
    "cell's last expression was not a Result and it did not yield, so nothing was "
    "returned. Wrap your final value (or yield each result):\n"
    "  Result.text('done')                       # same text to human and model\n"
    "  Result.ok('what happened')                # a quiet confirmation for a side effect\n"
    "  Result.of(value)                          # render any value richly for the human\n"
    "  Result(user_html='<b>hi</b>', llm_result='hi', llm_images=[fig])\n"
    "  yield Result.ok('step 1'); ...; yield Result.of(df)   # stream as you go\n"
    "Print is not a channel: stdout is not returned to the model and is hidden in "
    "the dashboard by default, so surface anything worth seeing as a Result."
)

_YIELD_REQUIRED = (
    "python_exec: this cell uses `yield` but yielded no Result(...). Yield at "
    "least one Result(...) so the run declares what the human and model receive."
)

_YIELD_NOT_RESULT = (
    "python_exec: a yielded value was not a Result(...). Every top-level `yield` "
    "in a cell must yield a Result (Result.text/ok/of or Result(user_html=...))."
)

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
        try:
            _store.rename(_store_conn, id=job.id, name=name)
        except Exception:
            pass


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


class JobStillRunning(RuntimeError):
    """Raised by ``Job.result`` when the job has not finished yet.

    Reaching for a running job's result is the one job-polling footgun: a plain
    ``None`` reads as "finished with no value". This raises instead, so the
    confusion surfaces as a clear instruction to ``await`` it (or poll
    ``.done()``) rather than a silent wrong answer.
    """


class Job:
    """A single ``python_exec`` execution: an awaitable handle over the asyncio
    task running the code, with its captured output, result, and status."""

    def __init__(self, code: str, name: str | None = None, budget: float = 15.0, kind: str = "cell"):
        self.id = uuid.uuid4().hex[:8]
        self.code = code
        self.name = name or self.id
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
        self._buf: list[str] = []
        self._buflen = 0
        # Rich outputs (mime bundles) display()-ed while this job runs.
        self._displays: list[dict] = []
        self.task: asyncio.Task | None = None
        # Set by the SIGUSR2 wedge watchdog so _runner can tell its interrupt from
        # a KeyboardInterrupt the user's own code raised.
        self.interrupted_by_watchdog = False

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

    def running(self) -> bool:
        return self.status == "running"

    def done(self) -> bool:
        """True once the job has finished (done, error, or cancelled). Pair it
        with `.result`, which only yields a value once the job is done."""
        return self.status != "running"

    @property
    def ok(self) -> bool:
        """True if the job finished successfully (no error, not cancelled)."""
        return self.status == "done"

    @property
    def result(self):
        """This run's final value -- the `Result` the cell produced (or the one
        auto-wrapped from a bare displayable final expression like a DataFrame).

        Accessing it while the job is still running raises `JobStillRunning`,
        instead of returning a misleading `None`: background the work, then
        `await jobs['id']` to get the result, or poll `.done()` / `.running()`
        first. Once finished this is exactly what `await jobs['id']` yields.

        ``Job.result`` is a **property** (not a method), but since ``Result``
        is callable, both ``job.result`` and ``job.result()`` hand back the
        value. The returned Result's ``.text`` attribute is the rendered model
        text (same as ``.llm_result``), so ``job.result.text[-100:]`` pages it."""
        if self.running():
            dur = time.time() - self.started
            raise JobStillRunning(
                f"job {self.id} is still running ({dur:.1f}s); "
                f"`await jobs['{self.id}']` to get its result, "
                f"or check `.done()` / `.running()` first"
            )
        return self._result

    def cancel(self) -> "Job":
        if self.task is not None and not self.task.done():
            self.task.cancel()
        return self

    async def wait(self, timeout: float | None = None) -> "Job":
        """Wait until this job finishes, or up to ``timeout`` seconds, and return
        the job (check ``.done()`` / ``.status`` / ``.result`` on it). Unlike
        ``await jobs['<id>']`` it never raises on a slow job -- it just returns
        the still-running handle at the deadline -- so one cell replaces a
        sleep-and-poll loop: ``(await jobs['ab12'].wait(30)).status``."""
        if self.task is not None and not self.task.done():
            await asyncio.wait({self.task}, timeout=timeout)
        return self

    def __await__(self):
        # `await jobs['id']` should yield the job's result, but the runner task
        # returns None, so wait for it then hand back the captured result.
        async def _await_result():
            await self.task
            return self._result

        return _await_result().__await__()

    def __repr__(self) -> str:
        dur = (self.ended or time.time()) - self.started
        at = f" L{line}" if self.running() and (line := _current_line(self)) else ""
        head = f"<Job {self.id} ({self.name}) [{self.status}{at}] {dur:.2f}s>"
        out = self.tail(800)
        return head + ("\n" + out if out else "")


def _llm_text(value):
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
    return _safe_repr(value)


def _result_from_text(cls, value, *, html: str | None = None) -> "Result":
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

    def __get__(self, obj, objtype=None):
        if obj is None:
            return types.MethodType(_result_from_text, objtype)
        return obj.llm_result or ""


class Result:
    """Split a cell's final value into a human view and a model view.

    Every ``python_exec`` cell must END with one of these (the kernel rejects a
    cell whose last expression is not a Result, so a run always declares what the
    human sees and what the model gets back). The dashboard renders
    ``user_html`` (a rich HTML view for the human watching); the model's tool
    result receives ``llm_result`` (concise text) plus any ``llm_images``. The
    two never cross: the human is not shown the model's text, and the model does
    not pay tokens for the HTML render.

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
    text as a fallback for plain hosts.

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
    def __init__(self, *values, user_html=None, llm_result=None, llm_images=None):
        llm_result = _llm_text(llm_result)
        if user_html is not None:
            self.user_html = user_html
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
        self.llm_result = built.llm_result
        self.llm_images = list(llm_images) if llm_images else built.llm_images

    # ``Result.text("hi")`` constructs (the classic constructor); on an instance,
    # ``.text`` is the rendered model text (same as ``.llm_result``) -- see
    # _TextDescriptor. The two meanings used to collide: ``(await job).text``
    # returned the bound constructor, a guessing-game error when inspecting a
    # finished job.
    text = _TextDescriptor()

    @classmethod
    def ok(cls, message: str = "done") -> "Result":
        """A quiet confirmation for a side-effecting cell (an import, a cancel, a
        terminal keystroke) that has no value to return."""
        msg = str(message)
        user = f'<div class="ix-ok">\u2713 {_escape_html(msg)}</div>'
        return cls(user_html=user, llm_result=msg)

    @classmethod
    def of(cls, value, *, llm_result: str | None = None) -> "Result":
        """Wrap any value: render it richly for the human (a DataFrame as a
        table, a figure as an image, anything else as its display HTML or repr)
        and hand the model concise text. For a polars DataFrame the model text is
        the frame as compact, untruncated CSV (the human still gets the styled
        HTML table), so a wide or long-stringed frame is never clipped to the
        agent the way the boxed text repr clips it. Override with ``llm_result``."""
        if isinstance(value, Result):
            # An existing Result is already split into its two views: copy it
            # faithfully (keeping llm_images) instead of rebuilding it from its
            # display bundle, which would drop the model image blocks. This also
            # preserves images when a nested Result is stacked below.
            return cls(
                user_html=value.user_html,
                llm_result=value.llm_result if llm_result is None else llm_result,
                llm_images=value.llm_images,
            )
        image_mime = _image_bytes_mime(value)
        if image_mime is not None:
            # Raw PNG/JPEG bytes (e.g. `await page.screenshot()`): show the human
            # the inline image and hand the model a real image block, not the
            # ~50k-char byte repr that would blow the result cap.
            img = _coerce_image(value)
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
            # A rich result type (fff GrepResult/SearchResult) that exposes a
            # polars frame: render that frame the same as a bare DataFrame -- a
            # styled table for the human, compact CSV for the model -- so the
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
            text_view = _safe_repr(value)
        if _is_polars_df(value):
            # A frame (incl. a dict/records value coerced above) renders as the
            # dashboard's styled table directly -- a table for the human, compact
            # CSV for you -- and works even without the IPython display formatter.
            try:
                import view as _view

                return cls(user_html=_view.df_html(value), llm_result=text_view)
            except Exception:
                user = f'<pre class="ix-result">{_escape_html(text_view)}</pre>'
                return cls(user_html=user, llm_result=text_view)
        bundle = _result_bundle(value)
        data = (bundle or {}).get("data", {})
        if "text/html" in data:
            user = data["text/html"]
        elif "image/png" in data:
            user = f'<img alt="" src="data:image/png;base64,{data["image/png"]}" />'
        elif "image/svg+xml" in data:
            user = data["image/svg+xml"]
        else:
            user = f'<pre class="ix-result">{_escape_html(text_view)}</pre>'
        return cls(user_html=user, llm_result=text_view)

    def _repr_mimebundle_(self, **_kwargs) -> dict:
        # IPython's display protocol: html is the human view (the dashboard
        # prefers it); IX_LLM_MIME carries the model's text+images, which the
        # server unpacks and the dashboard ignores; text/plain is the fallback.
        bundle: dict = {"text/html": self.user_html, "text/plain": self.llm_result or ""}
        images = [img for img in (_coerce_image(i) for i in self.llm_images) if img]
        if images:
            bundle[IX_LLM_MIME] = {"text": self.llm_result or "", "images": images}
        return bundle

    def __call__(self) -> "Result":
        """Calling a Result returns it unchanged. ``Job.result`` is a property,
        so ``jobs['id'].result()`` -- the natural method-call guess while
        polling a finished job -- used to die with "'Result' object is not
        callable". Property and call now both hand over the value."""
        return self

    def __repr__(self) -> str:
        # Plain-text fallback (the stored result repr, non-rich hosts): the model
        # view, never the HTML.
        return self.llm_result or ""


def _as_frame_if_tabular(value):
    """A mapping (a config dict, counts) or a list of mappings (records) is
    tabular: render it as a polars frame -- a styled table for the human, compact
    CSV for you -- rather than a raw dict/list repr. Anything else is returned
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
            try:
                return pl.DataFrame(
                    {"key": [str(k) for k in value], "value": pl.Series([dict(v) for v in vals])}
                )
            except Exception:
                pass
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
        # for the human, compact CSV for you. (Lists of mappings are records above.)
        try:
            return pl.DataFrame({"value": list(value)})
        except Exception:
            try:
                return pl.DataFrame({"value": [_safe_repr(v) for v in value]})
            except Exception:
                return value
    return value


def _is_rich_element(value) -> bool:
    """True if ``value`` carries its own rich view (a DataFrame, a figure/image,
    an htpy element, or a Result), so flattening it into a one-column frame would
    throw that view away. Plain scalars and containers are not rich."""
    return isinstance(value, Result) or _is_polars_df(value) or _is_displayable(value)


def _is_multi_rich(value) -> bool:
    """True for a non-empty list/tuple that carries at least one rich element, so
    ``Result.of`` should stack each element's view instead of coercing the whole
    sequence to a single table. A list/tuple of plain scalars (or of mappings)
    stays tabular -- only a sequence mixing in a DataFrame/figure/Result needs the
    stacked treatment."""
    return isinstance(value, (list, tuple)) and bool(value) and any(_is_rich_element(v) for v in value)


def _result_from_values(values, *, llm_result=None):
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
    all live resources updating in place. The resource closes itself (leaves the
    sidebar) when its ``alive`` predicate reports the source is gone.
    """

    def __init__(self, id, title, kind, render, alive=None):
        self.id = id
        self.title = title
        self.kind = kind
        self._render = render
        self._alive = alive
        self.status = "live"
        self.created = time.time()
        self.html = ""
        self.error: str | None = None

    def closed(self) -> bool:
        return self.status == "closed"

    def close(self) -> "Resource":
        """Close the resource so the sidebar drops it on the next tick."""
        self.status = "closed"
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

    async def render_html(self) -> str:
        """The current HTML for this resource (awaits the render if it is async)."""
        out = self._render() if callable(self._render) else self._render
        if inspect.iscoroutine(out):
            out = await out
        return out if isinstance(out, str) else str(out)

    def __repr__(self) -> str:
        return f"<Resource {self.id} ({self.title}) [{self.status}] {self.kind}>"


def register_resource(
    source=None, *, title=None, render=None, id=None, kind="html", alive=None
) -> Resource:
    """Register a live HTML resource for the dashboard sidebar.

    Pass a ``render`` callable returning the current HTML (sync or async), or a
    ``source`` object the runtime renders by calling its ``resource_html()`` /
    ``to_html()`` (whichever it has). ``alive`` is an optional predicate; when it
    returns False the resource closes itself and leaves the sidebar. Returns the
    :class:`Resource` handle (call ``.close()`` to remove it explicitly).
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
    res = Resource(rid, str(title), kind, render, alive)
    resources[rid] = res
    return res


jobs: dict[str, Job] = {}
resources: dict[str, Resource] = {}


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

    def _render(self, value, title: str | None) -> list:
        bundle = _result_bundle(value)
        if bundle is not None and bundle.get("data"):
            return [bundle]
        return [{"data": {"text/plain": _safe_repr(value)}, "metadata": {}}]

    def _find(self, key) -> int:
        """Resolve an int index or a string id to a list index, or -1."""
        if isinstance(key, int):
            return key if -len(self._items) <= key < len(self._items) else -1
        for i, cell in enumerate(self._items):
            if cell.id == key:
                return i
        return -1

    def add(self, value, *, title: str | None = None, id: str | None = None) -> str:
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

    def set(self, key, value, *, title: str | None = None) -> str:
        """Replace the cell at ``key`` (an int index or a string id) in place."""
        idx = self._find(key)
        if idx < 0:
            raise KeyError(f"no cell {key!r}")
        self._items[idx].outputs = self._render(value, title)
        if title is not None:
            self._items[idx].title = title
        self._rev += 1
        return self._items[idx].id

    def remove(self, key) -> None:
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

    def __setitem__(self, key, value) -> None:
        if isinstance(key, str):
            self.add(value, id=key)
        else:
            self.set(key, value)

    def __getitem__(self, key) -> _Cell:
        idx = self._find(key)
        if idx < 0:
            raise KeyError(f"no cell {key!r}")
        return self._items[idx]

    def __len__(self) -> int:
        return len(self._items)

    def __iter__(self):
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
        try:
            _store.replace_cells(_store_conn, rows)
            self._synced = self._rev
        except Exception:
            # Best-effort mirror: a store write must not raise into user code.
            pass


cells = Cells()


# AST node types that open their own scope: a `yield` (or a name binding) inside
# one of these belongs to that inner scope, not the cell's top level.
_NESTED_SCOPES = (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda, ast.ClassDef)


def _has_toplevel_yield(nodes) -> bool:
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


def _compile(code: str, filename: str) -> tuple[str, "types.CodeType"]:
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


def _compile_generator(code: str, filename: str) -> "types.CodeType":
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


def _stdout_hint(job: "Job") -> str:
    """A suffix for the Result-contract error when the cell also printed: stdout
    never reaches the model, so the bare "you must return a Result" message leaves
    a printing agent unsure what happened to its output. Show a preview of what
    was printed and the one-line fix, turning a silent dead-end into a nudge."""
    printed = job.output.strip()
    if not printed:
        return ""
    limit = 1500
    if len(printed) > limit:
        printed = f"{printed[:limit]}\n... [+{len(printed) - limit} more chars in jobs['{job.id}'].output]"
    return (
        "\n\nThis cell printed to stdout, which the model never receives. To send "
        f"this text, return it (e.g. `Result.text(...)`) or page jobs['{job.id}'].output. "
        f"What the cell printed:\n{printed}"
    )


def _display_result(result: "Result") -> None:
    """Show one yielded Result to both audiences. The IPython display goes onto
    the running job's captured outputs (the dashboard) and out on iopub (the
    model's tool result), the same path the trailing Result takes \u2014 so a
    yielding cell needs no separate plumbing."""
    from IPython.display import display

    try:
        display(result)
    except Exception:
        # Rich display is best-effort; a formatter failure must not abort the run.
        pass


def _is_displayable(value) -> bool:
    """True if a bare final expression value is rich enough to auto-wrap in
    ``Result.of`` (so ``df`` on the last line just works), False if returning it
    is the print-like anti-pattern the Result contract nudges away from.

    Displayable = it already knows how to render itself: an IPython rich repr
    (a polars DataFrame, a ``view.Code``, ...), an htpy-style ``__html__``, or a
    figure/image that renders through a registered formatter. Plain scalars,
    ``str``/``bytes``, and the container types (dict/list/tuple/set) are NOT --
    those still fail the contract, to keep pushing key/value data toward a
    DataFrame and confirmations toward ``Result.ok``.
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
    return module.startswith("matplotlib") or module.startswith("PIL")


def _current_line(job: "Job") -> int | None:
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


def _error_line(exc: BaseException, job: "Job") -> int | None:
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


async def _runner(job: Job, ns: dict) -> None:
    token = _ix_current.set(job)
    if _store is not None and _store_conn is not None:
        try:
            _store.start(
                _store_conn,
                id=job.id,
                name=job.name,
                code=job.code,
                started_at=job.started,
                budget=job.budget,
                kind=job.kind,
            )
        except Exception:
            # Best-effort logging: a store write must never abort the job.
            pass
    try:
        # Compile inside the runner so a SyntaxError is recorded as a job error
        # (status + traceback in the store/dashboard) instead of escaping __ix_run.
        mode, code_obj = _compile(job.code, f"<job {job.id}>")
        ns.pop("__ix_result", None)
        if mode == "gen":
            # A yielding cell streams results: drain the async generator and
            # display each yielded Result so it reaches the human (the job's
            # captured outputs) and the model (iopub) as it is produced. The
            # yields ARE the results, so there is no trailing-Result requirement.
            exec(code_obj, ns)
            agen = ns.pop("__ix_cell__")()
            job._aobj = agen  # sampled by _current_line while suspended
            emitted = 0
            async for item in agen:
                if not isinstance(item, Result):
                    job.status = "error"
                    job.error = _YIELD_NOT_RESULT
                    job._append(job.error)
                    break
                _display_result(item)
                emitted += 1
            else:
                # Loop ran to completion (no non-Result break).
                if emitted == 0:
                    job.status = "error"
                    job.error = _YIELD_REQUIRED + _stdout_hint(job)
                    job._append(job.error)
                else:
                    job.status = "done"
            # The results were displayed as they streamed; there is no single
            # trailing value to return.
            job._result = None
        else:
            maybe = eval(code_obj, ns)
            if inspect.iscoroutine(maybe):
                job._aobj = maybe  # sampled by _current_line while suspended
                await maybe
            value = ns.pop("__ix_result", None)
            if not isinstance(value, Result) and _is_displayable(value):
                # A bare final value that already knows how to render itself (a
                # DataFrame, a figure, a view.Code, an htpy element) is wrapped
                # in Result.of, so `df` on the last line just works. Plain
                # scalars / dicts / None fall through to the contract error below.
                value = Result.of(value)
            job._result = value
            if isinstance(value, Result):
                job.status = "done"
            else:
                # Enforce the Result contract: a non-yielding cell must END with a
                # Result (or a value that renders as one) so a run always declares
                # what the human sees and what the model gets. A bare scalar (or a
                # side-effecting cell returning None) is a failed run with an
                # instructive message, not a silent pass.
                job.status = "error"
                job.error = _RESULT_REQUIRED + _stdout_hint(job)
                job._append(job.error)
                job._result = None
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
        else:
            # The user's own code raised KeyboardInterrupt; keep its real
            # traceback (trimmed to the cell's frames) and the failing line.
            job.error = _user_traceback(_kexc)
            job.error_line = _error_line(_kexc, job)
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
        job._append(job.error)
    finally:
        job.ended = time.time()
        _ix_current.reset(token)
        _persist_final(job)
        _mark_snapshot_dirty()


def _persist_final(job: Job) -> None:
    if _store is None or _store_conn is None:
        return
    try:
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
        )
    except Exception:
        # Best-effort logging: persisting the final status must not raise during cleanup.
        pass


def _cell_bindings(job: Job) -> dict:
    """The live value each of the cell's identifiers is bound to, snapshotted now
    that the job has finished. Read off the shared user namespace (the same one
    the code ran in), so the dashboard can show inlay hints and hover values that
    reflect the actual objects. Best-effort: a failure here just means no hints."""
    ns = _user_ns if _user_ns is not None else globals()
    try:
        from .introspect import cell_bindings

        return cell_bindings(job.code, ns)
    except Exception:
        return {}


def _safe_repr(value) -> str:
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
# CSV handed back to the agent so a million-row frame cannot flood its context.
_DF_LLM_ROWS = 200


def _is_polars_df(value) -> bool:
    """True for a polars DataFrame, by duck typing. runtime.py stays import-light
    (polars is the user's to bring), so it never imports polars to check."""
    return (
        type(value).__module__.split(".", 1)[0] == "polars"
        and hasattr(value, "write_csv")
        and hasattr(value, "columns")
        and hasattr(value, "height")
    )


def _frame_view(value):
    """A non-DataFrame value that opts into the table protocol by exposing
    ``_ix_to_frame_()`` returning a polars DataFrame (e.g. an fff ``GrepResult``
    or ``SearchResult``). Returns that frame, else None. Lets a rich result type
    render as the styled table for the human and compact CSV for the model,
    instead of falling back to its one-line summary repr."""
    hook = getattr(value, "_ix_to_frame_", None)
    if hook is None:
        return None
    try:
        frame = hook()
    except Exception:
        return None
    return frame if _is_polars_df(frame) else None


def _df_llm_text(df) -> str:
    """A polars DataFrame as compact text for the model: a shape + dtype header
    then CSV, with cell values never truncated (only the row count is bounded by
    ``_DF_LLM_ROWS``). CSV is denser than the boxed repr and drops no value, so
    the agent reads the real data instead of a width-clipped table."""
    try:
        schema = ", ".join(f"{name}:{dtype}" for name, dtype in zip(df.columns, df.dtypes))
        rows, cols = df.shape
        body = df.head(_DF_LLM_ROWS).write_csv().rstrip("\n")
        more = f"\n... ({rows - _DF_LLM_ROWS} more rows)" if rows > _DF_LLM_ROWS else ""
        return f"shape: ({rows}, {cols}) | {schema}\n{body}{more}"
    except Exception:
        # An exotic frame that resists write_csv falls back to its plain repr.
        return _safe_repr(df)


def _png_bytes(img) -> bytes:
    """Encode a Pillow image as an optimized PNG (lossless)."""
    import io

    if img.mode not in ("RGB", "RGBA", "L"):
        img = img.convert("RGBA")
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def _jpeg_bytes(img, quality: int) -> bytes:
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
    except (ValueError, base64.binascii.Error):
        return {"mime": mime, "data": b64}
    return _encode_image_bytes(raw, mime)


def _image_bytes_mime(value) -> str | None:
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


def _coerce_image(value) -> dict | None:
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
        if len(s) < 4096 and os.path.isfile(s):
            try:
                raw = open(s, "rb").read()
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
            except Exception:
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
    if not job.running() and job._result is not None:
        bundle = _result_bundle(job._result)
        if bundle is not None:
            outs.append(bundle)
    return outs


_tui_mod = None
_tui_probed = False
_vmkit_mod = None
_vmkit_probed = False


def _tui_module():
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


def _tui_renderer(term):
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


def _vmkit_module():
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


def _vmkit_renderer(driver):
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

    def add(where: str, name: str, obj, summary: str | None = None) -> None:
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
        except Exception:
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
    for lib_name in registry.LIBRARIES:
        sig = lib_name
        summary = "bundled library -- import and use it directly (help() / its own docs)"
        try:
            mod = __import__(lib_name)
            version = getattr(mod, "__version__", "")
            if version:
                sig = f"{lib_name} {version}"
            doc = (inspect.getdoc(mod) or "").strip().split("\n", 1)[0]
            if doc:
                summary = doc
        except Exception:
            pass
        rows.append({"where": "library", "name": lib_name, "kind": "library", "sig": sig, "summary": summary})
    return rows


def doc(obj) -> "Result":
    """The signature and docstring of any object, RETURNED (not printed) as a
    Result -- so the documented "everything through Result" path also works for
    reading docs. ``help()`` only writes to stdout (not your channel) and returns
    ``None``, so ``Result(help(x))`` shows nothing; use ``doc(fff.grep)`` instead.
    Pair it with `api()`: `api('grep')` to find a name, `doc(fff.grep)` to read it."""
    name = getattr(obj, "__name__", None) or type(obj).__name__
    sig = ""
    if callable(obj):
        try:
            sig = f"{name}{inspect.signature(obj)}"
        except (ValueError, TypeError):
            sig = name if inspect.isclass(obj) else f"{name}(...)"
    body = inspect.getdoc(obj) or "(no docstring)"
    return Result.text(f"{sig}\n\n{body}" if sig else body)


def api(filter: str | None = None):
    """A live catalog of every helper the kernel gives you: the always-present
    namespace builtins (`Result`, `cells`, `jobs`, `sh`, ...) and the public
    surface of each bundled module (`fff`, `view`, `nix`, `fleet`, ...), each with
    its signature and a one-line summary. Call `api()` to discover what exists
    instead of guessing names or grepping source; pass `filter` to match a
    substring against the name, summary, or module.

    Returns a polars DataFrame (filter/sort it further, e.g.
    `api().filter(pl.col("where") == "fff")`), or plain text if polars is absent.
    """
    rows = _api_rows()
    if filter:
        q = filter.lower()
        rows = [
            r for r in rows
            if q in r["name"].lower() or q in r["summary"].lower() or q in r["where"].lower()
        ]
    try:
        import polars as _pl

        return _pl.DataFrame(
            rows,
            schema={"where": _pl.Utf8, "name": _pl.Utf8, "kind": _pl.Utf8, "sig": _pl.Utf8, "summary": _pl.Utf8},
        )
    except Exception:
        width = max((len(r["sig"]) for r in rows), default=0)
        return "\n".join(f'{r["where"]:>6}  {r["sig"]:<{width}}  {r["summary"]}' for r in rows)


_TYPEERROR_CALL_RE = re.compile(
    r"^(\w+)\(\) (got an unexpected keyword argument|missing \d+ required (keyword-only argument|positional argument))"
)


def _type_error_hint(exc: TypeError) -> str:
    """Return a one-line signature hint when *exc* is a call-binding TypeError.

    Matches errors like:
      - ``grep() got an unexpected keyword argument 'max_results'``
      - ``grep() missing 1 required keyword-only argument: 'mode'``

    Looks the callable up in the user namespace (and a set of well-known module
    prefixes like ``fff.``) so the hint shows the live signature. Returns an
    empty string on any failure so callers can unconditionally append it.
    """
    try:
        msg = str(exc)
        m = _TYPEERROR_CALL_RE.match(msg)
        if not m:
            return ""
        func_name = m.group(1)
        # Resolve the callable: check user namespace and well-known module attrs.
        ns = _user_ns if _user_ns is not None else globals()
        obj = ns.get(func_name)
        if obj is None:
            # Try module-qualified names (e.g. the error says "grep" but the
            # callable lives as fff.grep in the namespace).
            for mod_name in ("fff", "view", "nix", "fleet"):
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
        return f"\nHint: the signature is {func_name}{sig}; see doc({func_name})."
    except Exception:
        return ""


async def _sweep_resources() -> None:
    """Render every live resource to the store; close the ones whose source died."""
    if _store is None or _store_conn is None:
        return
    _discover_tui_resources()
    _discover_vmkit_resources()
    now = time.time()
    for res in list(resources.values()):
        if not res.alive():
            try:
                _store.close_resource(_store_conn, id=res.id, updated_at=now)
            except Exception:
                # Best-effort: a store write must not kill the loop.
                pass
            resources.pop(res.id, None)
            continue
        status = "live"
        try:
            # Bound each render so one wedged source cannot stall the whole loop.
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
        try:
            _store.upsert_resource(
                _store_conn,
                id=res.id,
                title=res.title,
                kind=res.kind,
                html=res.html,
                status=status,
                created_at=res.created,
                updated_at=now,
            )
        except Exception:
            # Best-effort live render: a store write must not kill the loop.
            pass


async def _flusher() -> None:
    """Throttled background loop: persist every running job's output tail and
    re-render every live resource to the store so the dashboard shows both live.
    One loop for all jobs and resources (cheap)."""
    if _store is None or _store_conn is None:
        return
    while True:
        await asyncio.sleep(0.5)
        for job in list(jobs.values()):
            if job.running():
                job.line = _current_line(job)
                try:
                    _store.update_output(
                        _store_conn, job.id, job.output, job._displays or None, line=job.line
                    )
                except Exception:
                    # Best-effort live output: a store write must not kill the loop.
                    pass
        await _sweep_resources()
        cells._sync()
        if _SESSION and _snapshot_dirty and not _snapshot_busy and not _restoring:
            # Fire-and-forget so a multi-second dump of a big namespace never
            # stalls the live-output mirroring this loop exists for.
            asyncio.ensure_future(_snapshot_tick())


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


def _dill():
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
    except Exception:
        # Leave the previous checkpoint in place; replay covers the gap on
        # reopen. The next finished cell re-marks dirty, so this also cannot
        # spin on a persistently failing dump.
        pass
    finally:
        _snapshot_last = time.time()
        _snapshot_busy = False


async def __ix_snapshot() -> "Result":
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
            named = pickle.loads(snap["blob"])
        except Exception as exc:
            print(f"session restore: checkpoint decode failed ({exc}); replaying the full log")
            named, snap = {}, None
        else:
            for name, payload in named.items():
                try:
                    ns[name] = loader.loads(payload)
                    restored.append(name)
                except Exception:
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
        try:
            await _snapshot_now()
        except Exception:
            pass
    parts = [f"{len(restored)} names restored instantly"]
    if snap is not None and snap.get("skipped"):
        parts.append(f"{len(snap['skipped'])} not in checkpoint ({', '.join(s['name'] for s in snap['skipped'][:10])})")
    if load_failed:
        parts.append(f"{len(load_failed)} failed to load ({', '.join(load_failed[:10])})")
    parts.append(f"{len(rows)} cells replayed")
    if replay_failed:
        parts.append(f"{len(replay_failed)} replays failed ({', '.join(replay_failed[:10])})")
    print("session restore: " + "; ".join(parts))


async def __ix_run(code: str, budget: float = 15.0, name: str | None = None, kind: str = "cell") -> Job:
    """Run ``code`` as a task; wait up to ``budget`` for it; return the Job either
    way (done, or still running in the background)."""
    ns = _user_ns if _user_ns is not None else globals()
    job = Job(code, name, budget=budget, kind=kind)
    jobs[job.id] = job
    job.task = asyncio.ensure_future(_runner(job, ns))
    await asyncio.wait({job.task}, timeout=budget)
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
        "status": job.status,
        "running": job.running(),
        "output": job.tail(_SUMMARY_CHARS),
        "output_chars": len(job.output),
        "result": None if job._result is None else _safe_repr(job._result),
        "result_chars": len(_result_text(job)),
        "error": job.error,
        # Where a still-running job is right now (cell line), so a budget-expired
        # reply can say not just "running" but "running, on line N".
        "line": _current_line(job) if job.running() else None,
    }


def history(n: int = 20) -> "Result":
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
        try:
            display(job._result)
        except Exception:
            # Rich display is best-effort; failures must not block the summary.
            pass


async def __ix_exec(code: str, budget: float = 15.0, name: str | None = None) -> None:
    """The MCP server's per-call entrypoint: run with a budget, emit the summary."""
    job = await __ix_run(code, budget=budget, name=name)
    _emit(job)


def _existing_file(value) -> pathlib.Path | None:
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


def _tilde(path) -> str:
    """An absolute path with the home directory collapsed to ``~`` for a compact,
    privacy-friendly note (``/Users/me/.ix/trace/x`` -> ``~/.ix/trace/x``)."""
    text = str(path)
    home = str(pathlib.Path.home())
    if text == home:
        return "~"
    if text.startswith(home + os.sep):
        return "~" + text[len(home):]
    return text


# File-type icons for the read note, rendered as inline SVG so the dashboard
# (which trusts agent HTML/SVG -- see RichOutput.svelte) shows a real document
# glyph with the extension on a colored ribbon, not an emoji. The color is keyed
# by lowercased extension; any unknown extension still gets the document shape
# with a neutral ribbon, so every file reads as a file.
_EXT_COLORS = {
    "py": "#3776ab", "rs": "#dea584", "go": "#00add8",
    "js": "#f1e05a", "mjs": "#f1e05a", "cjs": "#f1e05a",
    "ts": "#3178c6", "tsx": "#3178c6", "jsx": "#f1e05a",
    "json": "#cbcb41", "jsonl": "#cbcb41", "ndjson": "#cbcb41",
    "toml": "#9c4221", "yaml": "#cb171e", "yml": "#cb171e",
    "ini": "#8a8a92", "cfg": "#8a8a92", "conf": "#8a8a92", "env": "#8a8a92",
    "nix": "#7e7eff",
    "md": "#519aba", "rst": "#519aba", "txt": "#9aa0a6",
    "sh": "#89e051", "bash": "#89e051", "zsh": "#89e051", "fish": "#89e051", "nu": "#3aa675",
    "html": "#e44d26", "htm": "#e44d26", "xml": "#e37933",
    "css": "#563d7c", "scss": "#c6538c",
    "csv": "#41b883", "tsv": "#41b883", "parquet": "#41b883",
    "log": "#9aa0a6", "lock": "#e3c15b", "sql": "#dad8d8", "pdf": "#e02d2d",
    "png": "#a074c4", "jpg": "#a074c4", "jpeg": "#a074c4",
    "gif": "#a074c4", "svg": "#ffb13b", "webp": "#a074c4",
}
_NAMED_EXTS = {"dockerfile": "docker", "makefile": "make"}
_DEFAULT_EXT_COLOR = "#8a8a92"


def _file_icon_svg(path, *, px: int = 16) -> str:
    """An inline-SVG file icon for the read note: a document with a folded corner
    and the extension on a category-colored ribbon. Works for any extension."""
    name = pathlib.Path(path).name
    ext = (_NAMED_EXTS.get(name.lower()) or pathlib.Path(name).suffix.lstrip(".") or "txt").lower()
    color = _EXT_COLORS.get(ext, _DEFAULT_EXT_COLOR)
    label = _escape_html(ext[:4].upper())
    width = round(px * 0.8)
    return (
        f'<svg width="{width}" height="{px}" viewBox="0 0 40 50" fill="none" '
        f'xmlns="http://www.w3.org/2000/svg" style="vertical-align:-3px;flex:none">'
        f'<path d="M5 2h21l9 9v35a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z" '
        f'fill="#23232a" stroke="#3a3a42" stroke-width="1.5"/>'
        f'<path d="M26 2l9 9h-9z" fill="#3a3a42"/>'
        f'<rect x="3" y="30" width="34" height="14" rx="2" fill="{color}"/>'
        f'<text x="20" y="40.5" font-family="ui-monospace,Menlo,monospace" font-size="11" '
        f'font-weight="700" text-anchor="middle" fill="#111">{label}</text></svg>'
    )


def _value_icon_svg(*, px: int = 16) -> str:
    """An inline-SVG icon for a read of a kernel value (not a file): braces, to
    distinguish a value/object dump from a file read."""
    width = round(px * 0.8)
    return (
        f'<svg width="{width}" height="{px}" viewBox="0 0 40 50" fill="none" '
        f'xmlns="http://www.w3.org/2000/svg" style="vertical-align:-3px;flex:none">'
        f'<rect x="3" y="6" width="34" height="38" rx="4" fill="#23232a" '
        f'stroke="#3a3a42" stroke-width="1.5"/>'
        f'<text x="20" y="33" font-family="ui-monospace,Menlo,monospace" font-size="18" '
        f'font-weight="700" text-anchor="middle" fill="#9aa0a6">{{ }}</text></svg>'
    )


async def __ix_read(target, start=None, end=None) -> "Result":
    """Read a file (or evaluate a kernel value) FOR THE MODEL, quietly.

    Returns a Result whose ``llm_result`` is the full text the model receives and
    whose ``user_html`` is a one-line note the human sees, so a large read informs
    the model without flooding the dashboard. ``target`` is read as a file when it
    names an existing file, otherwise evaluated as a Python expression in the user
    namespace (e.g. ``jobs['ab12'].output``, a variable you bound). ``start`` and
    ``end`` select a 1-based inclusive line range. Backs the ``read`` MCP tool.
    """
    ns = _user_ns if _user_ns is not None else globals()
    value = None
    path = _existing_file(target)
    if path is None:
        # Not a file on disk: evaluate the expression. If the VALUE is a string
        # naming an existing file (`os.path.join(...)`, a variable holding a
        # path), the same file rule applies to it -- an expression yielding a
        # path reads the file, never echoes the path string back.
        value = eval(target, ns) if isinstance(target, str) else target
        path = _existing_file(value)
    if path is not None:
        # Off the loop: a large file read is blocking I/O, the one thing that
        # freezes every other job on the shared event loop.
        full = await asyncio.to_thread(path.read_text, errors="replace")
        label = _tilde(path)
        icon = _file_icon_svg(path)
    else:
        full = value if isinstance(value, str) else _safe_repr(value)
        label = target if isinstance(target, str) else _safe_repr(target)
        icon = _value_icon_svg()
    lines = full.splitlines()
    total = len(lines)
    if start is not None or end is not None:
        lo = max((start or 1) - 1, 0)
        hi = total if end is None else min(end, total)
        selected = lines[lo:hi]
        body = "\n".join(selected)
        span = f"lines {lo + 1}-{lo + len(selected)} of {total}"
    else:
        body = full
        span = f"{total} lines"
    note = f"read {label} \u00b7 {span}, {len(body)} chars"
    user = f'<div class="ix-ok">{icon} {_escape_html(note)}</div>'
    return Result(user_html=user, llm_result=body)



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
    """Make the bundled view objects render richly in the dashboard.

    Two gaps in a bare ipykernel: it only wires the inline matplotlib png
    formatter after ``%matplotlib inline``, and IPython's ``text/html`` formatter
    consults only ``_repr_html_``, never the ``__html__`` protocol that htpy (the
    bundled HTML builder) and markupsafe implement. Without the latter, an htpy
    element handed to ``cells.add``/``Result.of`` falls back to its ``repr``
    (``<Element '<div ...>...'>``) instead of rendering. Register both lazily by
    type name so importing matplotlib or htpy stays the user's choice."""
    try:
        png = shell.display_formatter.formatters["image/png"]
        png.for_type_by_name("matplotlib.figure", "Figure", _figure_png)
    except Exception:
        # No display formatter (non-IPython host) or a matplotlib too old to wire.
        pass
    try:
        html = shell.display_formatter.formatters["text/html"]
        html.for_type_by_name("htpy._elements", "BaseElement", lambda el: el.__html__())
        html.for_type_by_name("markupsafe", "Markup", str)
    except Exception:
        # No display formatter, or an htpy/markupsafe layout this does not match.
        pass


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
        _trace_file = open(trace_path, "w")  # truncates any stale dump from a prior kernel
        # enable() handles fatal signals (SIGSEGV/SIGABRT) -> stderr; register()
        # adds the on-demand SIGUSR1 all-thread dump the kernel_trace tool reads.
        faulthandler.enable()
        faulthandler.register(signal.SIGUSR1, file=_trace_file, all_threads=True, chain=False)

    def _break(signum, frame):
        # Only raise while a job is on the stack; a stray signal to an idle kernel
        # must not blow up the event loop. The handler runs in the interrupted
        # frame's context, so it sees the running job's ContextVar. Flag the job so
        # _runner can tell this watchdog interrupt from a KeyboardInterrupt the
        # user's own code raised (which must keep its real traceback).
        job = _ix_current.get()
        if job is not None:
            job.interrupted_by_watchdog = True
            raise KeyboardInterrupt("ix: cell exceeded its budget while blocking the event loop")

    try:
        signal.signal(signal.SIGUSR2, _break)
    except ValueError:
        # signal.signal only works on the main thread; the in-process unit tests
        # call install() off the main thread. Only the real kernel needs rescue.
        pass


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
    target["resources"] = resources
    target["Resource"] = Resource
    target["register_resource"] = register_resource
    target["__ix_run"] = __ix_run
    target["__ix_exec"] = __ix_exec
    target["__ix_read"] = __ix_read
    target["__ix_snapshot"] = __ix_snapshot
    target["__ix_restore"] = __ix_restore
    target["DASHBOARD_URL"] = os.environ.get("IX_MCP_DASHBOARD_URL", "")
    # `sh` is a bundled, callable module (see packages/mcp/src/sh). Bind it here
    # so `await sh(cmd)` works with no import, the way Result/cells/jobs do; an
    # explicit `import sh` returns the same object, so both styles agree.
    try:
        import sh as _sh_module
        target["sh"] = _sh_module
    except Exception:
        # Outside the bundled interpreter the module may be absent; skip it.
        pass
    # Pre-bind the two most-reached-for bundled modules so `fff.grep(...)` and
    # `view.ls(...)` work with no import, the way Result/cells/jobs/sh do (an
    # explicit `import fff` returns the same object, so both styles agree). Both
    # are already imported at startup (01-ix-polars installs view's renderer, which
    # imports fff), so binding them here costs nothing; heavier modules (nix,
    # fleet, search) stay import-on-demand to keep the namespace lean.
    for _mod_name in registry.preimport_names():
        try:
            target[_mod_name] = __import__(_mod_name)
        except Exception:
            pass
    # The kernel is async-first and polars-first: nearly every session reaches
    # for asyncio (ensure_future / sleep), json (every CLI's --json output), and
    # pl within its first cells, and a NameError on `asyncio` in an async kernel
    # is pure friction (observed twice in one 2026-06-10 session). Bound like
    # sh/fff/view; an explicit import returns the same module.
    target["asyncio"] = asyncio
    target["json"] = json
    try:
        import polars as _polars_mod

        target["pl"] = _polars_mod
    except Exception:
        pass
    target["api"] = api

    # Everything in the namespace up to here is the runtime's own surface plus
    # the kernel preamble -- not user state. Session checkpoints cover only the
    # names bound after this line (see _snapshot_candidates).
    global _baseline_names
    _baseline_names = frozenset(target)

    try:
        asyncio.get_event_loop().create_task(_flusher())
    except RuntimeError:
        # No loop yet (e.g. install from a sync context): the flusher is optional.
        pass
