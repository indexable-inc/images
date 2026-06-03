"""Type stubs for the native PyO3 module.

Hand-maintained to mirror packages/search-py/src/lib.rs. Keep in sync
when changing the binding. `semantic` and `grep` each return a native
asyncio-awaitable coroutine produced by pyo3-async-runtimes; awaiting it drives
the underlying tokio future.
"""

from __future__ import annotations

from collections.abc import Awaitable
from typing import TypedDict

__version__: str

class Hit(TypedDict):
    """One scored search result from the shared corpus store."""

    path: str
    score: float
    start_line: int | None
    num_lines: int | None
    text: str
    source: str

def semantic(
    query: str,
    top_k: int = ...,
    store: str | None = ...,
    base_url: str | None = ...,
    rerank: bool = ...,
    web: bool = ...,
    source: list[str] | None = ...,
    not_source: list[str] | None = ...,
    repo: str | None = ...,
    user: list[str] | None = ...,
    host: list[str] | None = ...,
    project: list[str] | None = ...,
    agentic: bool = ...,
) -> Awaitable[list[Hit]]: ...
def grep(
    pattern: str,
    top_k: int = ...,
    store: str | None = ...,
    base_url: str | None = ...,
    case_sensitive: bool = ...,
    source: list[str] | None = ...,
    not_source: list[str] | None = ...,
    repo: str | None = ...,
    user: list[str] | None = ...,
    host: list[str] | None = ...,
    project: list[str] | None = ...,
) -> Awaitable[list[Hit]]: ...
