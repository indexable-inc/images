"""First MCP tool use starts the shared dashboard hub once."""

from __future__ import annotations

import asyncio

import pytest


def test_start_dashboard_once(monkeypatch: pytest.MonkeyPatch) -> None:
    from ix_notebook_mcp import tools

    calls: list[bool] = []

    def fake_ensure_shared_dashboard(*, open_browser: bool = False) -> dict[str, object]:
        calls.append(open_browser)
        return {"url": "http://127.0.0.1:8080/"}

    monkeypatch.setattr("ix_notebook_mcp.cli.ensure_shared_dashboard", fake_ensure_shared_dashboard)
    monkeypatch.setattr(tools, "_dashboard_started", False)

    asyncio.run(tools._start_dashboard_once())
    asyncio.run(tools._start_dashboard_once())

    assert calls == [False]


def _dashboard_state() -> dict[str, object]:
    return {"url": "http://127.0.0.1:8080/"}


class _TtyStdout:
    def __init__(self) -> None:
        self.text = ""

    def write(self, value: str) -> int:
        self.text += value
        return len(value)

    def flush(self) -> None:
        pass

    def isatty(self) -> bool:
        return True


def test_dashboard_cli_prints_without_browser_by_default(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from ix_notebook_mcp import cli

    opened: list[str] = []
    stdout = _TtyStdout()

    monkeypatch.setattr(cli, "ensure_shared_dashboard", _dashboard_state)
    monkeypatch.setattr(cli.webbrowser, "open", opened.append)
    monkeypatch.setattr(cli.sys, "stdout", stdout)

    assert cli._dashboard() == 0
    assert stdout.text == "http://127.0.0.1:8080/\n"
    assert opened == []


def test_dashboard_cli_open_is_explicit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from ix_notebook_mcp import cli

    opened: list[str] = []
    stdout = _TtyStdout()

    monkeypatch.setattr(cli, "ensure_shared_dashboard", _dashboard_state)
    monkeypatch.setattr(cli.webbrowser, "open", opened.append)
    monkeypatch.setattr(cli.sys, "stdout", stdout)

    assert cli._dashboard(open_browser=True) == 0
    assert opened == ["http://127.0.0.1:8080/"]
