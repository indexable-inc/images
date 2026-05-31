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
    """One scored search result, scoped to the queried checkout."""

    path: str
    score: float
    start_line: int | None
    num_lines: int | None
    text: str
    is_web: bool

def semantic(
    query: str,
    path: str,
    top_k: int = ...,
    store: str | None = ...,
    base_url: str | None = ...,
    no_sync: bool = ...,
    rerank: bool = ...,
    web: bool = ...,
) -> Awaitable[list[Hit]]: ...
def grep(
    pattern: str,
    path: str,
    top_k: int = ...,
    store: str | None = ...,
    base_url: str | None = ...,
    no_sync: bool = ...,
    case_sensitive: bool = ...,
) -> Awaitable[list[Hit]]: ...
