from __future__ import annotations

import ast
import asyncio
import base64
import contextlib
import io
import json
import os
import sys
import traceback
from collections.abc import Callable
from typing import Any

# Cap on images returned per call, so a cell that opens many figures cannot
# balloon one response. The Rust side enforces the same ceiling.
MAX_IMAGES = 8

# Cap on characters returned per text field (stdout, stderr, result). A cell
# that prints a large file or reprs a huge object would otherwise stream
# straight into the caller's context window. Truncation is explicit: the marker
# names the dropped count so a clipped field never reads as complete.
MAX_OUTPUT_CHARS = 100_000

# Compile every snippet with this flag so `await` is legal at the top level.
# Without it, `await x` outside a function raises SyntaxError. CPython's own
# `python -m asyncio` REPL drives top-level await the same way: compile with the
# flag, then run the resulting coroutine on a loop.
# https://docs.python.org/3/library/asyncio-runner.html#asyncio-cli
_AWAIT_FLAG = ast.PyCF_ALLOW_TOP_LEVEL_AWAIT


class PythonSession:
    def __init__(self) -> None:
        self.globals: dict[str, object] = {}
        # Objects to render as images this call: anything passed to the injected
        # `display()`, plus the eval result. Reset at the start of each capture.
        self._displayed: list[object] = []
        self._last_result: object = None
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

    def evaluate(self, expression: str) -> dict[str, object]:
        def run() -> str:
            code = compile(expression, "<ix-mcp eval>", "eval", flags=_AWAIT_FLAG)
            result = self._drive(eval(code, self.globals))
            self._last_result = result
            return repr(result)

        return self.capture(run)

    def execute(self, source: str) -> dict[str, object]:
        def run() -> str:
            code = compile(source, "<ix-mcp exec>", "exec", flags=_AWAIT_FLAG)
            self._drive(eval(code, self.globals))
            return ""

        return self.capture(run)

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

    def capture(self, run: Callable[[], str]) -> dict[str, object]:
        stdout = io.StringIO()
        stderr = io.StringIO()
        ok = True
        self._displayed = []
        self._last_result = None

        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                value = run()
            except Exception:
                ok = False
                value = ""
                traceback.print_exc()

        return {
            "ok": ok,
            "stdout": _truncate(stdout.getvalue()),
            "stderr": _truncate(stderr.getvalue()),
            "result": _truncate(value),
            "images": self._collect_images(),
        }


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


def main() -> None:
    # Detach the JSON-RPC channel from fd 0 before any session code runs. The
    # Rust server talks to this worker over stdin/stdout, but a child process
    # spawned from a session (subprocess.run([...])) inherits fd 0, so a
    # path-less `rg`/`cat`/`grep` would read this RPC pipe and block the whole
    # session forever. Read requests from a dup and point fd 0 at /dev/null so
    # inherited stdin returns EOF immediately instead of stealing the pipe.
    rpc_in = os.fdopen(os.dup(sys.stdin.fileno()), "r", encoding="utf-8")
    with open(os.devnull, "rb") as devnull:
        os.dup2(devnull.fileno(), sys.stdin.fileno())

    session = PythonSession()
    for line in rpc_in:
        response = handle_request(session, line)
        sys.stdout.write(json.dumps(response) + "\n")
        sys.stdout.flush()
        if response.get("close", False):
            return


def handle_request(session: PythonSession, line: str) -> dict[str, object]:
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
                response = session.evaluate(_string_field(request, "expression"))
            case "exec":
                response = session.execute(_string_field(request, "source"))
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
