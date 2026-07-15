"""The read tool's target expression supports top-level await (index#3139).

``__ix_read`` evaluated its target with plain ``eval()``, so an ``await ...``
target (e.g. ``await jobs['ab12']``) died with ``SyntaxError: 'await' outside
function``. The expression now compiles with ``ast.PyCF_ALLOW_TOP_LEVEL_AWAIT``
and its coroutine is awaited on the kernel loop, matching how cells support
top-level await (``_compile``).
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

import pytest

from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict[str, Any]) -> None:
    """A controlled shared namespace, mirroring what install() leaves behind."""
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


def test_await_target_evaluates(monkeypatch: pytest.MonkeyPatch) -> None:
    async def answer() -> int:
        await asyncio.sleep(0)
        return 41

    _wire(monkeypatch, {"answer": answer})
    result = asyncio.run(runtime.__ix_read("await answer() + 1"))
    assert result.llm_result == "42"


def test_awaited_value_naming_a_file_reads_the_file(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # The path rule applies to the RESOLVED value: an awaited expression whose
    # value names an existing file reads that file, never echoes the path.
    note = tmp_path / "note.txt"
    note.write_text("hello from disk\n")

    async def where() -> str:
        return str(note)

    _wire(monkeypatch, {"where": where})
    result = asyncio.run(runtime.__ix_read("await where()"))
    assert "hello from disk" in result.llm_result


def test_plain_expression_target_unchanged(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {"answer": 41})
    result = asyncio.run(runtime.__ix_read("answer + 1"))
    assert result.llm_result == "42"
