"""The read tool must fail loudly when the kernel cannot execute (index#2381).

A wedged or dead kernel used to make `read` return empty content,
indistinguishable from reading an empty file: agents in one session twice
misread a known non-empty file as empty exactly that way. Any state in which
the in-kernel `__ix_read` did not complete must raise `McpError` ("kernel
unavailable"), never pose as empty success; only a COMPLETED read may report
emptiness, because an empty file is a real answer.

Pure unit tests over a stubbed kernel bridge: no kernel boots, no sockets bind
(the darwin sandbox denies loopback binds), so this runs anywhere.
"""

from __future__ import annotations

import asyncio
import pytest
from mcp.shared.exceptions import McpError

from ix_notebook_mcp import tools
from ix_notebook_mcp.kernel import _wedged_summary


class _StubKernel:
    """Stands in for the kernel bridge: `read` only calls `python_exec` on it."""

    def __init__(self, outputs: list[dict], summary: dict | None) -> None:
        self._outputs = outputs
        self._summary = summary
        self.code: str | None = None

    async def python_exec(
        self,
        code: str,
        budget: float,
        name: str | None = None,
        session: str | None = None,
        topic: str | None = None,
    ) -> tuple[list[dict], dict | None]:
        self.code = code
        return self._outputs, self._summary


def _wire(
    monkeypatch: pytest.MonkeyPatch, outputs: list[dict], summary: dict | None
) -> _StubKernel:
    """Point the read tool at a stub kernel and disarm the per-call gates."""
    stub = _StubKernel(outputs, summary)

    async def gate(*args: object, **kwargs: object) -> None:
        return None

    monkeypatch.setattr(tools, "_start_dashboard_once", gate)
    monkeypatch.setattr(tools, "_identify_client_once", gate)
    monkeypatch.setattr(tools, "_require_session_name", gate)
    monkeypatch.setattr(tools, "current_kernel", lambda: stub)
    return stub


def test_wedged_kernel_errors_instead_of_empty(monkeypatch: pytest.MonkeyPatch) -> None:
    """The regression itself: a wedge summary (the kernel's event loop is held
    by a blocked cell, so `__ix_read` never ran) must raise, not return []."""
    summary = _wedged_summary(30.0, 5.0, 35.0, outcome="restart_pending")
    _wire(monkeypatch, [], summary)

    with pytest.raises(McpError, match="kernel unavailable") as excinfo:
        asyncio.run(tools.read("~/notes.md"))
    message = str(excinfo.value)
    assert "nothing was read" in message
    # The wedge summary's own diagnosis rides along so the caller sees WHY.
    assert "blocked the kernel's event loop" in message


def test_no_summary_and_no_output_errors(monkeypatch: pytest.MonkeyPatch) -> None:
    """A bridge that returns neither output nor a job summary (dead kernel on
    an older build, stale runtime without `__ix_read`) must also raise."""
    _wire(monkeypatch, [], None)

    with pytest.raises(McpError, match="kernel unavailable"):
        asyncio.run(tools.read("~/notes.md"))


def test_cancelled_read_errors(monkeypatch: pytest.MonkeyPatch) -> None:
    """A terminal-but-incomplete state (cancelled) produced no text either;
    it must not read as an empty file."""
    _wire(monkeypatch, [], {"id": "ab12", "status": "cancelled", "running": False})

    with pytest.raises(McpError, match="cancelled"):
        asyncio.run(tools.read("~/notes.md"))


def test_completed_empty_read_still_reports_no_output(monkeypatch: pytest.MonkeyPatch) -> None:
    """An empty file is a real answer: a COMPLETED read with no text keeps the
    quiet '(no output)' placeholder and does NOT raise."""
    _wire(monkeypatch, [], {"id": "ab12", "status": "done", "running": False})

    content = asyncio.run(tools.read("empty-file.txt"))
    assert [item.text for item in content] == ["(no output)"]


def test_running_read_points_at_its_job(monkeypatch: pytest.MonkeyPatch) -> None:
    """A read that outlives its foreground budget on a healthy kernel names
    the live job instead of posing as empty output."""
    _wire(monkeypatch, [], {"id": "ab12", "status": "running", "running": True})

    content = asyncio.run(tools.read("big.expression"))
    assert len(content) == 1
    assert "jobs['ab12']" in content[0].text
    assert "still running" in content[0].text


def test_in_kernel_error_returns_traceback(monkeypatch: pytest.MonkeyPatch) -> None:
    """An error raised BY the read (bad expression, unreadable path) is the
    useful answer: it comes back as content, unchanged behavior."""
    _wire(monkeypatch, [], {"id": "ab12", "status": "error", "error": "NameError: name 'nope' is not defined"})

    content = asyncio.run(tools.read("nope"))
    assert len(content) == 1
    assert "NameError" in content[0].text


def test_successful_read_returns_its_text(monkeypatch: pytest.MonkeyPatch) -> None:
    """The happy path is untouched: a completed read's text is the content."""
    outputs = [
        {
            "output_type": "execute_result",
            "data": {"text/plain": "line one"},
            "metadata": {},
            "execution_count": 1,
        }
    ]
    stub = _wire(monkeypatch, outputs, {"id": "ab12", "status": "done", "running": False})

    content = asyncio.run(tools.read("notes.md", start=1, end=1))
    assert [item.text for item in content] == ["line one"]
    # The tool routed through the in-kernel __ix_read helper.
    assert stub.code is not None
    assert "__ix_read" in stub.code
