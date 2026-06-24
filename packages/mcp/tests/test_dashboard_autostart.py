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

    assert calls == [True]
