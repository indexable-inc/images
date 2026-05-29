from __future__ import annotations

import ast
import asyncio
import contextlib
import io
import json
import sys
import traceback
from collections.abc import Callable
from typing import Any

# Compile every snippet with this flag so `await` is legal at the top level.
# Without it, `await x` outside a function raises SyntaxError. CPython's own
# `python -m asyncio` REPL drives top-level await the same way: compile with the
# flag, then run the resulting coroutine on a loop.
# https://docs.python.org/3/library/asyncio-runner.html#asyncio-cli
_AWAIT_FLAG = ast.PyCF_ALLOW_TOP_LEVEL_AWAIT


class PythonSession:
    def __init__(self) -> None:
        self.globals: dict[str, object] = {"__name__": "__ix_mcp__"}
        # One persistent loop for the whole session. asyncio.run() would create
        # and close a fresh loop per call, orphaning any async resource (client,
        # connection pool, socket) bound to it; keeping one loop lets those
        # survive across requests, which is the point of a persistent session.
        self.loop = asyncio.new_event_loop()
        asyncio.set_event_loop(self.loop)

    def evaluate(self, expression: str) -> dict[str, object]:
        def run() -> str:
            code = compile(expression, "<ix-mcp eval>", "eval", flags=_AWAIT_FLAG)
            return repr(self._drive(eval(code, self.globals)))

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
        self.globals.clear()
        self.globals["__name__"] = "__ix_mcp__"
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

        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            try:
                value = run()
            except Exception:
                ok = False
                value = ""
                traceback.print_exc()

        return {
            "ok": ok,
            "stdout": stdout.getvalue(),
            "stderr": stderr.getvalue(),
            "result": value,
        }


def main() -> None:
    session = PythonSession()
    for line in sys.stdin:
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
