from __future__ import annotations

import ast
import asyncio
import base64
import codecs
import io
import json
import os
import re
import sys
import tempfile
import threading
import traceback
from collections.abc import Callable
from typing import Any

# Cap on images returned per call, so a cell that opens many figures cannot
# balloon one response. The Rust side enforces the same ceiling.
MAX_IMAGES = 8

# Cap on rich-HTML tables shipped to the dashboard per call (one per displayed
# DataFrame / last-expression result). HTML goes only to the human dashboard, not
# the model's tool result, so the ceiling guards the producer snapshot, not the
# context window.
MAX_HTML = 8

# Rows materialized into a single dashboard HTML table. The human view is a
# preview, so a huge frame ships its first rows with a "showing N of M" caption
# rather than streaming megabytes of cells into the Loro doc.
MAX_HTML_ROWS = 500

# Cap on characters returned per text field (stdout, stderr, result). A cell
# that prints a large file or reprs a huge object would otherwise stream
# straight into the caller's context window. Truncation is explicit: the marker
# names the dropped count so a clipped field never reads as complete.
MAX_OUTPUT_CHARS = 100_000

# Cap on bytes read back from a capture file before decoding, so a cell whose
# subprocess writes gigabytes cannot balloon worker memory. We read a little
# over the character cap and let `_truncate` mark the clip.
MAX_CAPTURE_BYTES = 4 * MAX_OUTPUT_CHARS

# How often the streaming watcher polls the capture file for new output while a
# cell is still running, in seconds. Fast enough to feel live on the dashboard,
# slow enough that a chatty loop does not flood the RPC channel with tiny
# partials. The watcher exists so a long-running or never-returning cell (e.g. a
# `while True` loop) shows output as it is produced; the final response, sent
# when the cell returns, still carries the complete captured output.
STREAM_INTERVAL_SECS = 0.2

# Compile every snippet with this flag so `await` is legal at the top level.
# Without it, `await x` outside a function raises SyntaxError. CPython's own
# `python -m asyncio` REPL drives top-level await the same way: compile with the
# flag, then run the resulting coroutine on a loop.
# https://docs.python.org/3/library/asyncio-runner.html#asyncio-cli
_AWAIT_FLAG = ast.PyCF_ALLOW_TOP_LEVEL_AWAIT

# Synthetic filenames for the user's own snippet (see `evaluate`/`execute`). Output
# is line-attributed only when the writing frame belongs to the user's code, not a
# library it called.
_USER_FILES = frozenset({"<ix-mcp eval>", "<ix-mcp exec>"})

# Sends one JSON message to the Rust server. The same callback delivers both the
# interim `partial` chunks (from the streaming watcher) and the final response,
# all serialized under one lock so concurrent writes never interleave on the
# wire. See `main`.
Emit = Callable[[dict[str, object]], None]


class _LineTee:
    """Tee `sys.stdout` so each write is attributed to the user source line that
    produced it, while still delegating to the real (fd-backed) stream — the
    canonical stdout capture and any subprocess output are untouched.

    This powers *inline-trace execution*: rendering each ``print`` beside the line
    of code that emitted it. Precedent: Bret Victor's "Inventing on Principle",
    Light Table's instarepl, Python Tutor, and marimo. ``print`` is a C builtin, so
    the nearest Python frame at ``write`` time is the user's calling line; we record
    its line number only when that frame is the user's own snippet.
    """

    def __init__(self, orig: Any, trace: list[tuple[int, str]]) -> None:
        self._orig = orig
        self._trace = trace

    def write(self, s: str) -> int:
        if s:
            try:
                frame = sys._getframe(1)  # noqa: SLF001 — deliberate caller introspection
                if frame.f_code.co_filename in _USER_FILES:
                    self._trace.append((frame.f_lineno, s))
            except Exception:
                pass
        return self._orig.write(s)

    def flush(self) -> None:
        self._orig.flush()

    def __getattr__(self, name: str) -> Any:
        # Delegate everything else (encoding, isatty, fileno, ...) to the real stream.
        return getattr(self._orig, name)


def _coalesce_trace(trace: list[tuple[int, str]]) -> list[dict[str, object]]:
    """Merge adjacent writes from the same line and cap total size, returning
    ``[{"line": int, "text": str}]`` in emission order for the inline-trace view."""
    out: list[dict[str, object]] = []
    budget = MAX_OUTPUT_CHARS
    for line, text in trace:
        if budget <= 0:
            break
        if len(text) > budget:
            text = text[:budget]
        budget -= len(text)
        if out and out[-1]["line"] == line:
            out[-1]["text"] = f"{out[-1]['text']}{text}"
        else:
            out.append({"line": line, "text": text})
    return out


def _stream_capture(fd: int, stop: threading.Event, emit: Emit, request_id: object) -> None:
    """Tail the stdout capture file while a cell runs, emitting each new chunk as
    a `partial` message so the dashboard renders output live.

    Reads with ``os.pread`` at an explicit offset: ``fd`` is the same open file
    description that ``capture`` has ``dup2``'d onto fd 1, and a positional read
    never moves the shared write offset, so tailing cannot disturb the canonical
    capture. Decoding is incremental so a multi-byte character split across two
    polls is not mangled. Streaming stops at ``MAX_OUTPUT_CHARS`` — the same cap
    the final response uses — so a runaway producer cannot flood the channel.
    """
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    offset = 0
    streamed = 0

    def drain() -> None:
        nonlocal offset, streamed
        try:
            size = os.fstat(fd).st_size
        except OSError:
            return
        while offset < size and streamed < MAX_OUTPUT_CHARS:
            chunk = os.pread(fd, min(size - offset, 65536), offset)
            if not chunk:
                break
            offset += len(chunk)
            text = decoder.decode(chunk)
            if not text:
                continue
            if streamed + len(text) > MAX_OUTPUT_CHARS:
                text = text[: MAX_OUTPUT_CHARS - streamed]
            streamed += len(text)
            emit({"id": request_id, "partial": True, "stdout": text})

    # Poll until the cell finishes (`stop` is set), then drain once more so the
    # tail written between the last poll and completion is not lost.
    while not stop.wait(STREAM_INTERVAL_SECS):
        drain()
    drain()


class PythonSession:
    def __init__(self) -> None:
        self.globals: dict[str, object] = {}
        # Objects to render as images this call: anything passed to the injected
        # `display()`, plus the eval result. Reset at the start of each capture.
        self._displayed: list[object] = []
        self._last_result: object = None
        # Whether the one-time polars compact-repr config has been applied.
        self._polars_compact_applied: bool = False
        self._reset_globals()
        # One persistent loop for the whole session. asyncio.run() would create
        # and close a fresh loop per call, orphaning any async resource (client,
        # connection pool, socket) bound to it; keeping one loop lets those
        # survive across requests, which is the point of a persistent session.
        self.loop = asyncio.new_event_loop()
        asyncio.set_event_loop(self.loop)

    def _reset_globals(self) -> None:
        self.globals.clear()
        self.globals["__name__"] = "__ix_mcp__"
        # A Jupyter-style `display()` so explicit `display(obj)` (and several
        # per cell) are captured as images, not just the cell's final value.
        self.globals["display"] = self._display

    def _display(self, *objects: object, **_kwargs: object) -> None:
        self._displayed.extend(objects)

    def _collect_images(self) -> list[dict[str, str]]:
        candidates = list(self._displayed)
        if self._last_result is not None:
            candidates.append(self._last_result)
        images = [image for obj in candidates if (image := _object_png(obj)) is not None]
        images.extend(_matplotlib_pngs())
        return images[:MAX_IMAGES]

    def _collect_html(self) -> list[str]:
        """Rich HTML for each displayed object and the eval result, in order.

        A DataFrame (polars or pandas) renders as a self-contained sortable grid;
        any other object that implements the Jupyter `_repr_html_` / mimebundle
        protocol falls back to that. Returned only to the human dashboard (see
        `worker_response_content` in main.rs), never to the model's tool result,
        so a wide table costs the operator a scroll, not the context window."""
        candidates = list(self._displayed)
        if self._last_result is not None:
            candidates.append(self._last_result)
        docs = [doc for obj in candidates if (doc := _object_html_doc(obj)) is not None]
        return docs[:MAX_HTML]

    # polars repr knobs this session bounds by default; also the keys it checks to
    # see whether the user already configured the repr (then it leaves it alone).
    _POLARS_REPR_KEYS = ("POLARS_FMT_MAX_ROWS", "POLARS_FMT_MAX_COLS", "POLARS_FMT_STR_LEN")

    def _apply_polars_compact(self) -> None:
        """Bound polars' text repr once polars is in use, so an existing
        `print(df)` / `o.report()` stays compact in the model's captured stdout.
        The dashboard still gets the full table as HTML.

        Applied at most once per session, and only when the user has not already
        set any of these repr knobs themselves: `pl.Config` is process-global, so
        re-applying would silently undo a `pl.Config.set_tbl_rows(...)` the user
        ran in an earlier cell. Checked at each cell's start (polars is usually not
        imported at session reset), but the once-flag and the user-set check make
        it a default the user's own config always wins over."""
        if self._polars_compact_applied:
            return
        pl = sys.modules.get("polars")
        if pl is None:
            return
        self._polars_compact_applied = True
        try:
            # `state(if_set=True)` lists only the knobs explicitly set; if the user
            # already touched any repr knob, respect their whole repr config.
            already_set = pl.Config.state(if_set=True)
            if any(key in already_set for key in self._POLARS_REPR_KEYS):
                return
            pl.Config.set_tbl_rows(20)
            pl.Config.set_tbl_cols(20)
            pl.Config.set_fmt_str_lengths(50)
        except Exception:
            # A polars build without `state`/one of these setters must not break
            # capture; fall back to leaving the repr untouched.
            pass

    def evaluate(
        self, expression: str, emit: Emit | None = None, request_id: object = None
    ) -> dict[str, object]:
        def run() -> str:
            code = compile(expression, "<ix-mcp eval>", "eval", flags=_AWAIT_FLAG)
            result = self._drive(eval(code, self.globals))
            self._last_result = result
            return repr(result)

        return self.capture(run, emit, request_id)

    def execute(
        self, source: str, emit: Emit | None = None, request_id: object = None
    ) -> dict[str, object]:
        def run() -> str:
            code = compile(source, "<ix-mcp exec>", "exec", flags=_AWAIT_FLAG)
            self._drive(eval(code, self.globals))
            return ""

        return self.capture(run, emit, request_id)

    def _drive(self, value: object) -> object:
        # Code compiled with the await flag returns a coroutine only when it
        # actually contains top-level await; otherwise it runs eagerly and
        # returns its normal value (None for exec-mode statements). Driving the
        # coroutine on the session loop makes top-level await block until the
        # result is ready, the same way synchronous code blocks the worker.
        if asyncio.iscoroutine(value):
            return self.loop.run_until_complete(value)
        return value

    def reset(self) -> dict[str, object]:
        self._reset_globals()
        # Keep the loop. Clearing globals already drops the caller's async
        # resources, and recreating the loop would invalidate any reference the
        # caller stored elsewhere.
        return {"ok": True, "stdout": "", "stderr": "", "result": "session reset"}

    def close(self) -> None:
        if not self.loop.is_closed():
            self.loop.close()

    def capture(
        self, run: Callable[[], str], emit: Emit | None = None, request_id: object = None
    ) -> dict[str, object]:
        # Capture at the file-descriptor level, not just `sys.stdout`: redirect
        # fds 1 and 2 to temp files so a subprocess the cell spawns
        # (`subprocess.run(["echo", "hi"])`) is captured in order alongside
        # Python `print`, instead of leaking to the worker's real stdout. The
        # JSON-RPC channel lives on a separate fd (see `main`), so this never
        # touches the protocol.
        ok = True
        value = ""
        self._displayed = []
        self._last_result = None
        self._apply_polars_compact()

        # Inline-trace execution: each stdout write is paired with the user source
        # line that produced it (see `_LineTee`), so the dashboard can render output
        # beside the line that printed it.
        trace: list[tuple[int, str]] = []

        out_file = tempfile.TemporaryFile()
        err_file = tempfile.TemporaryFile()
        sys.stdout.flush()
        sys.stderr.flush()
        saved_out = os.dup(1)
        saved_err = os.dup(2)
        saved_stdout = sys.stdout
        # Stream stdout to the caller while the cell runs, so a long-running or
        # never-returning cell is visible live instead of only on completion. The
        # watcher tails `out_file` (which fd 1 is redirected to) and emits chunks.
        stop = threading.Event()
        watcher: threading.Thread | None = None
        try:
            os.dup2(out_file.fileno(), 1)
            os.dup2(err_file.fileno(), 2)
            # Tee for line attribution; writes still pass through to fd 1, so the
            # canonical capture below (and subprocess output) is unaffected.
            sys.stdout = _LineTee(saved_stdout, trace)
            if emit is not None:
                # Flush each printed line to the capture file promptly so the
                # watcher can tail it. fd 1 is not a tty here, so the TextIOWrapper
                # is block-buffered by default and a `print` per second would sit
                # in the buffer, never reaching disk until the cell ends. Line
                # buffering makes each newline flush through to `out_file`.
                # (Subprocess output goes straight to fd 1, so it already streams.)
                try:
                    saved_stdout.reconfigure(line_buffering=True)
                except (ValueError, OSError):
                    pass
                watcher = threading.Thread(
                    target=_stream_capture,
                    args=(out_file.fileno(), stop, emit, request_id),
                    daemon=True,
                )
                watcher.start()
            try:
                value = run()
            except Exception:
                ok = False
                value = ""
                traceback.print_exc()
            finally:
                # Flush user output to the capture file before stopping the
                # watcher, so its final drain sees everything the cell wrote.
                sys.stdout.flush()
                sys.stderr.flush()
                stop.set()
                if watcher is not None:
                    watcher.join(timeout=1.0)
                sys.stdout = saved_stdout
        finally:
            os.dup2(saved_out, 1)
            os.dup2(saved_err, 2)
            os.close(saved_out)
            os.close(saved_err)

        return {
            "ok": ok,
            "stdout": _truncate(_read_capture(out_file)),
            "stderr": _truncate(_read_capture(err_file)),
            "result": _truncate(value),
            "images": self._collect_images(),
            "html": self._collect_html(),
            "trace": _coalesce_trace(trace),
        }


def _read_capture(handle: io.IOBase) -> str:
    """Read a capture temp file back as text and close it. Reads at most
    `MAX_CAPTURE_BYTES` so a runaway producer cannot exhaust memory; the decode
    replaces any byte sequence the cap split mid-character."""
    handle.seek(0)
    data = handle.read(MAX_CAPTURE_BYTES)
    handle.close()
    return data.decode("utf-8", "replace")


def _truncate(text: str, limit: int = MAX_OUTPUT_CHARS) -> str:
    if len(text) <= limit:
        return text
    return f"{text[:limit]}\n... [ix-mcp truncated {len(text) - limit} chars]"


def _object_png(obj: object) -> dict[str, str] | None:
    """Extract a PNG/JPEG for `obj` via the Jupyter rich-display protocol.

    Tries `_repr_mimebundle_()` first, then the per-format `_repr_png_` /
    `_repr_jpeg_` hooks. Covers `PIL.Image`, `IPython.display.Image`, matplotlib
    figures, and anything else implementing those methods.
    """
    bundle = _mime_bundle(obj)
    if bundle is not None:
        for mime in ("image/png", "image/jpeg"):
            data = bundle.get(mime)
            if data:
                return _as_b64(mime, data)
    for mime, method in (("image/png", "_repr_png_"), ("image/jpeg", "_repr_jpeg_")):
        hook = getattr(obj, method, None)
        if callable(hook):
            try:
                data = hook()
            except Exception:
                continue
            if data:
                return _as_b64(mime, data)
    return None


def _mime_bundle(obj: object) -> dict[str, object] | None:
    hook = getattr(obj, "_repr_mimebundle_", None)
    if not callable(hook):
        return None
    try:
        data = hook()
    except Exception:
        return None
    if isinstance(data, tuple):  # (bundle, metadata)
        data = data[0]
    return data if isinstance(data, dict) else None


def _as_b64(mime: str, data: object) -> dict[str, str]:
    # `_repr_png_` returns raw bytes; a MIME bundle stores image/png as a
    # base64 string already (the Jupyter convention), so pass strings through.
    encoded = data if isinstance(data, str) else base64.b64encode(bytes(data)).decode("ascii")
    return {"mime": mime, "base64": encoded}


def _matplotlib_pngs() -> list[dict[str, str]]:
    """Capture any open matplotlib figures as PNGs, so a bare `plt.plot(...)`
    returns an image without an explicit `display()`. Figures are closed after
    capture so they are not re-emitted on the next call."""
    plt = sys.modules.get("matplotlib.pyplot")
    if plt is None:
        return []
    images: list[dict[str, str]] = []
    try:
        for num in plt.get_fignums():
            buffer = io.BytesIO()
            plt.figure(num).savefig(buffer, format="png", bbox_inches="tight")
            images.append(
                {"mime": "image/png", "base64": base64.b64encode(buffer.getvalue()).decode("ascii")}
            )
        plt.close("all")
    except Exception:
        return images
    return images


def _object_html_doc(obj: object) -> str | None:
    """A self-contained HTML document for `obj`, or `None` if it has no HTML form.

    A DataFrame (polars or pandas) becomes a sortable grid built from its own
    columns and rows, independent of any `pl.Config` row/column cap. Anything else
    that implements the Jupyter rich-display protocol (`_repr_html_` or a
    `text/html` mimebundle) is wrapped in a minimal styled document so it mounts
    in the dashboard's sandboxed frame."""
    table = _table_data(obj)
    if table is not None:
        return _render_table_doc(table)
    fragment = _object_html(obj)
    if fragment is not None:
        return _wrap_html(fragment)
    return None


def _table_data(obj: object) -> dict[str, object] | None:
    """Columns, dtypes, and capped rows for a polars or pandas DataFrame.

    Duck-typed so neither library is imported here: a frame is recognized by its
    own already-imported type. Returns the full `height` so the renderer can note
    when more rows exist than the `MAX_HTML_ROWS` preview shows."""
    module = type(obj).__module__.split(".", 1)[0]
    try:
        if module == "polars" and hasattr(obj, "columns") and hasattr(obj, "rows"):
            columns = list(obj.columns)
            dtypes = [str(t) for t in obj.dtypes]
            height = int(obj.height)
            rows = obj.head(MAX_HTML_ROWS).rows()
            return {"columns": columns, "dtypes": dtypes, "rows": rows, "height": height}
        if module == "pandas" and hasattr(obj, "columns") and hasattr(obj, "itertuples"):
            columns = [str(c) for c in obj.columns]
            dtypes = [str(t) for t in obj.dtypes]
            height = int(len(obj))
            rows = list(obj.head(MAX_HTML_ROWS).itertuples(index=False, name=None))
            return {"columns": columns, "dtypes": dtypes, "rows": rows, "height": height}
    except Exception:
        # A frame whose data cannot be materialized (lazy errors, exotic dtypes)
        # must not break the run; it simply contributes no HTML pane.
        return None
    return None


# A cell is numeric (right-aligned, sorted as a number) when its dtype name says
# so. Covers polars (`Int64`, `Float32`, `UInt8`, `Decimal`) and pandas
# (`int64`, `float64`, `uint32`).
_NUMERIC_DTYPE = re.compile(r"^(u?int|float|decimal)", re.IGNORECASE)


def _render_table_doc(table: dict[str, object]) -> str:
    columns: list[str] = table["columns"]  # type: ignore[assignment]
    dtypes: list[str] = table["dtypes"]  # type: ignore[assignment]
    rows: list[tuple[object, ...]] = table["rows"]  # type: ignore[assignment]
    height: int = table["height"]  # type: ignore[assignment]

    numeric = [bool(_NUMERIC_DTYPE.match(d)) for d in dtypes]
    head = "".join(
        f'<th class="{"num" if numeric[i] else "txt"}" data-i="{i}">'
        f"{_html_escape(name)}<span class=dt>{_html_escape(dtypes[i])}</span></th>"
        for i, name in enumerate(columns)
    )
    body_rows = []
    for row in rows:
        cells = "".join(
            f'<td class="{"num" if numeric[i] else "txt"}">{_html_escape(_cell(v))}</td>'
            for i, v in enumerate(row)
        )
        body_rows.append(f"<tr>{cells}</tr>")
    body = "".join(body_rows)
    shown = len(rows)
    caption = f"{height} × {len(columns)}"
    if shown < height:
        caption += f"  ·  showing first {shown}"
    return _wrap_html(
        f'<table><caption>{_html_escape(caption)}</caption>'
        f"<thead><tr>{head}</tr></thead><tbody>{body}</tbody></table>{_SORT_JS}"
    )


def _cell(value: object) -> str:
    if value is None:
        return ""
    return str(value)


def _object_html(obj: object) -> str | None:
    """A `text/html` fragment for `obj` via the Jupyter rich-display protocol:
    a `text/html` mimebundle entry first, then `_repr_html_()`."""
    bundle = _mime_bundle(obj)
    if bundle is not None:
        html = bundle.get("text/html")
        if isinstance(html, str) and html:
            return html
    hook = getattr(obj, "_repr_html_", None)
    if callable(hook):
        try:
            html = hook()
        except Exception:
            return None
        if isinstance(html, str) and html:
            return html
    return None


def _html_escape(text: str) -> str:
    return (
        text.replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
    )


# Click a header to sort by that column; click again to reverse. Numeric columns
# (`th.num`) compare as numbers with blanks last, others as strings. Pure DOM, no
# dependency, so it runs in the dashboard's `allow-scripts` sandbox.
_SORT_JS = """<script>
(function(){
  var tb=document.querySelector('tbody'), ths=document.querySelectorAll('th');
  ths.forEach(function(th,i){
    th.addEventListener('click',function(){
      var asc=th.dataset.dir!=='asc'; ths.forEach(function(t){t.removeAttribute('data-dir')});
      th.dataset.dir=asc?'asc':'desc';
      var num=th.classList.contains('num');
      var rows=[].slice.call(tb.querySelectorAll('tr'));
      rows.sort(function(a,b){
        var x=a.children[i].textContent, y=b.children[i].textContent;
        if(num){var nx=parseFloat(x),ny=parseFloat(y);
          if(isNaN(nx))return 1; if(isNaN(ny))return -1; return asc?nx-ny:ny-nx;}
        return asc?x.localeCompare(y):y.localeCompare(x);
      });
      rows.forEach(function(r){tb.appendChild(r)});
    });
  });
})();
</script>"""


# Flat, square, monospace, dark+light — matches the dashboard's still aesthetic.
# A producer ships its own document into the sandboxed frame, so the styling is
# self-contained rather than inherited from the host page.
_HTML_STYLE = """<style>
:root{color-scheme:light dark}
html,body{margin:0;font:12px/1.5 'Berkeley Mono',ui-monospace,Menlo,monospace}
body{padding:8px;background:#fff;color:#111}
table{border-collapse:collapse;width:100%}
caption{text-align:left;padding:0 0 6px;opacity:.6;font-size:11px}
th,td{border:1px solid #d8d8d8;padding:2px 8px;white-space:nowrap;text-align:left}
td.num,th.num{text-align:right;font-variant-numeric:tabular-nums}
thead th{position:sticky;top:0;background:#f4f4f4;cursor:pointer;user-select:none;font-weight:600}
thead th:hover{background:#eaeaea}
th[data-dir=asc]::after{content:' ▲'}th[data-dir=desc]::after{content:' ▼'}
.dt{display:block;font-weight:400;opacity:.45;font-size:10px}
tbody tr:nth-child(even){background:#fafafa}
@media (prefers-color-scheme:dark){
  body{background:#0d0d0d;color:#e6e6e6}
  th,td{border-color:#262626}
  thead th{background:#161616}thead th:hover{background:#1f1f1f}
  tbody tr:nth-child(even){background:#121212}
}
</style>"""


def _wrap_html(fragment: str) -> str:
    """Wrap an HTML fragment in a minimal self-contained document with the
    dashboard's table styling, suitable for the sandboxed iframe."""
    return f"<!doctype html><meta charset=utf-8>{_HTML_STYLE}{fragment}"


def main() -> None:
    # Detach the JSON-RPC channel from fds 0 and 1 before any session code runs.
    # The Rust server talks to this worker over stdin/stdout, but a child process
    # a cell spawns (`subprocess.run([...])`) inherits both: a path-less
    # `rg`/`cat` would read this RPC pipe and block the session forever, and a
    # bare `echo` would write onto the response stream and desync the protocol.
    #
    # Read requests from a dup of fd 0 and point fd 0 at /dev/null so inherited
    # stdin returns EOF immediately. Write responses to a dup of fd 1 (`rpc_out`)
    # and point fd 1 at /dev/null, so the capture in `PythonSession.capture` is
    # the only thing that ever redirects fd 1, and user/subprocess output lands
    # there instead of on the wire.
    rpc_in = os.fdopen(os.dup(sys.stdin.fileno()), "r", encoding="utf-8")
    rpc_out = os.fdopen(os.dup(sys.stdout.fileno()), "w", encoding="utf-8")
    with open(os.devnull, "rb") as devnull_in, open(os.devnull, "wb") as devnull_out:
        os.dup2(devnull_in.fileno(), sys.stdin.fileno())
        os.dup2(devnull_out.fileno(), sys.stdout.fileno())

    # One lock guards every write to the RPC channel: the streaming watcher
    # (running on its own thread during a cell) and the main loop's final
    # response both go through `emit`, so their JSON lines never interleave.
    write_lock = threading.Lock()

    def emit(payload: dict[str, object]) -> None:
        line = json.dumps(payload)
        with write_lock:
            rpc_out.write(line + "\n")
            rpc_out.flush()

    session = PythonSession()
    for line in rpc_in:
        response = handle_request(session, line, emit)
        emit(response)
        if response.get("close", False):
            return


def handle_request(session: PythonSession, line: str, emit: Emit) -> dict[str, object]:
    try:
        request = json.loads(line)
        if not isinstance(request, dict):
            raise TypeError("request must be a JSON object")
        request_id = request.get("id")
        op = request.get("op")
        match op:
            case "ping":
                response: dict[str, object] = {"ok": True, "stdout": "", "stderr": "", "result": "session ready"}
            case "eval":
                response = session.evaluate(_string_field(request, "expression"), emit, request_id)
            case "exec":
                response = session.execute(_string_field(request, "source"), emit, request_id)
            case "reset":
                response = session.reset()
            case "close":
                session.close()
                response = {"ok": True, "stdout": "", "stderr": "", "result": "session closed", "close": True}
            case _:
                raise ValueError(f"unknown operation: {op}")
        response["id"] = request_id
        return response
    except Exception:
        stderr = traceback.format_exc()
        return {"id": None, "ok": False, "stdout": "", "stderr": stderr, "result": ""}


def _string_field(request: dict[str, Any], key: str) -> str:
    value = request.get(key)
    if isinstance(value, str):
        return value
    raise TypeError(f"{key} must be a string")


if __name__ == "__main__":
    main()
