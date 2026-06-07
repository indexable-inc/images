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
    "python_exec: every cell must END with a Result(...). This cell's last "
    "expression was not a Result, so nothing was returned. Wrap your final value:\n"
    "  Result.text('done')                       # same text to human and model\n"
    "  Result.ok('what happened')                # a quiet confirmation for a side effect\n"
    "  Result.of(value)                          # render any value richly for the human\n"
    "  Result(user_html='<b>hi</b>', llm_result='hi', llm_images=[fig])\n"
    "Curate the human's view of the most important results with cells.add(value)."
)

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
        """Last ``n`` chars of this job's captured output."""
        return self.output[-n:]

    def head(self, n: int = 2000) -> str:
        """First ``n`` chars of this job's captured output."""
        return self.output[:n]

    def slice(self, start: int = 0, end: int | None = None) -> str:
        """A character window ``output[start:end]``, for paging a large output a
        chunk at a time after `grep`/`lines` locates the region."""
        return self.output[start:end]

    def lines(self, start: int = 0, end: int | None = None) -> str:
        """Output lines ``[start:end]`` (0-based, ``end`` exclusive), numbered to
        match `grep`'s line numbers so you can jump straight to a region."""
        numbered = self.output.splitlines()
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
        src = self.output.splitlines()
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


@dataclasses.dataclass
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
    the model as a real image block. It is a mime bundle under the hood:
    ``text/html`` carries ``user_html`` and, when present, ``IX_LLM_MIME`` carries
    the model's text+images (unpacked by the server); ``text/plain`` carries the
    text as a fallback for plain hosts.
    """

    user_html: str
    llm_result: str = ""
    llm_images: list = dataclasses.field(default_factory=list)

    @classmethod
    def text(cls, value, *, html: str | None = None) -> "Result":
        """A Result that shows the same text to the human and the model. Pass
        ``html`` to give the human a richer view than the plain text."""
        body = value if isinstance(value, str) else _safe_repr(value)
        user = html if html is not None else f"<pre class=\"ix-result\">{_escape_html(body)}</pre>"
        return cls(user_html=user, llm_result=body)

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
        if llm_result is not None:
            text_view = llm_result
        elif _is_polars_df(value):
            text_view = _df_llm_text(value)
        else:
            text_view = _safe_repr(value)
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

    def __repr__(self) -> str:
        # Plain-text fallback (the stored result repr, non-rich hosts): the model
        # view, never the HTML.
        return self.llm_result or ""


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
        if isinstance(job.result, Result):
            job.status = "done"
        else:
            # Enforce the Result contract: every cell must END with a Result so a
            # run always declares what the human sees and what the model gets.
            # A bare value (or a side-effecting cell that returns None) is a
            # failed run with an instructive message rather than a silent pass.
            job.status = "error"
            job.error = _RESULT_REQUIRED
            job._append(job.error)
            job.result = None
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


def _coerce_image(value) -> dict | None:
    """Coerce one ``Result.llm_images`` item to ``{"mime", "data"}`` (base64),
    or None if it is not an image we can encode. Accepts raw PNG/JPEG bytes, a
    base64 / data-URI string, a path to an image file, a matplotlib Figure, or
    any object with ``_repr_png_`` / ``_repr_jpeg_`` (a PIL image, a plot)."""
    if value is None:
        return None
    # Raw bytes: sniff PNG vs JPEG by magic, default to PNG.
    if isinstance(value, (bytes, bytearray)):
        raw = bytes(value)
        mime = "image/jpeg" if raw[:3] == b"\xff\xd8\xff" else "image/png"
        return {"mime": mime, "data": base64.b64encode(raw).decode("ascii")}
    if isinstance(value, str):
        s = value.strip()
        if s.startswith("data:image/"):
            head, _, payload = s.partition(",")
            mime = head[5:].split(";", 1)[0] or "image/png"
            return {"mime": mime, "data": payload}
        # A filesystem path to an image.
        if len(s) < 4096 and os.path.isfile(s):
            try:
                raw = open(s, "rb").read()
            except OSError:
                return None
            mime = "image/jpeg" if s.lower().endswith((".jpg", ".jpeg")) else "image/png"
            return {"mime": mime, "data": base64.b64encode(raw).decode("ascii")}
        # Otherwise assume it is already base64-encoded PNG.
        return {"mime": "image/png", "data": s}
    # matplotlib Figure: render to PNG.
    if type(value).__module__.startswith("matplotlib") and hasattr(value, "savefig"):
        try:
            return {"mime": "image/png", "data": base64.b64encode(_figure_png(value)).decode("ascii")}
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
                return {"mime": mime, "data": base64.b64encode(bytes(out)).decode("ascii")}
            if isinstance(out, str):
                return {"mime": mime, "data": out}
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
                try:
                    _store.update_output(_store_conn, job.id, job.output, job._displays or None)
                except Exception:
                    # Best-effort live output: a store write must not kill the loop.
                    pass
        await _sweep_resources()
        cells._sync()


async def __ix_run(code: str, budget: float = 15.0, name: str | None = None) -> Job:
    """Run ``code`` as a task; wait up to ``budget`` for it; return the Job either
    way (done, or still running in the background)."""
    ns = _user_ns if _user_ns is not None else globals()
    job = Job(code, name)
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
    repr), used only to measure how much the inline summary leaves out."""
    if job.result is None:
        return ""
    return getattr(job.result, "llm_result", None) or _safe_repr(job.result)


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
        "result": None if job.result is None else _safe_repr(job.result),
        "result_chars": len(_result_text(job)),
        "error": job.error,
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
    # On success job.result is always a Result (the runner enforces it); display
    # it so the server can unpack the model view and the dashboard the human one.
    if job.status == "done" and job.result is not None:
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
    target["history"] = history
    target["Job"] = Job
    target["Result"] = Result
    target["cells"] = cells
    target["Cells"] = Cells
    target["resources"] = resources
    target["Resource"] = Resource
    target["register_resource"] = register_resource
    target["__ix_run"] = __ix_run
    target["__ix_exec"] = __ix_exec
    target["DASHBOARD_URL"] = os.environ.get("IX_MCP_DASHBOARD_URL", "")

    try:
        asyncio.get_event_loop().create_task(_flusher())
    except RuntimeError:
        # No loop yet (e.g. install from a sync context): the flusher is optional.
        pass
