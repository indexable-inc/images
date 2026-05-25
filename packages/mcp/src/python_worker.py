from __future__ import annotations

import contextlib
import io
import json
import sys
import traceback
from collections.abc import Callable
from typing import Any, cast


class PythonSession:
    def __init__(self) -> None:
        self.globals: dict[str, object] = {"__name__": "__ix_mcp__"}

    def evaluate(self, expression: str) -> dict[str, object]:
        def run() -> str:
            value = cast(object, eval(compile(expression, "<ix-mcp eval>", "eval"), self.globals))
            return repr(value)

        return self.capture(run)

    def execute(self, source: str) -> dict[str, object]:
        def run() -> str:
            exec(compile(source, "<ix-mcp exec>", "exec"), self.globals)
            return ""

        return self.capture(run)

    def reset(self) -> dict[str, object]:
        self.globals.clear()
        self.globals["__name__"] = "__ix_mcp__"
        return {"ok": True, "stdout": "", "stderr": "", "result": "session reset"}

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
