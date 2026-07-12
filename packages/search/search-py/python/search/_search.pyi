"""Type stubs for the native PyO3 module.

Hand-maintained to mirror packages/search/search-py/src/lib.rs. Keep in sync
when changing the binding. `semantic`, `grep`, and `recent` each return a
native asyncio-awaitable coroutine produced by pyo3-async-runtimes; awaiting it
drives the underlying tokio future and yields a `list[Hit]`.

These are the private bindings. The public `search.semantic` / `grep` /
`recent` (in `__init__.py`) wrap them: they are `async def`, so awaiting one
yields a `polars.DataFrame` (one row per `Hit`) instead of the raw list.
"""

from __future__ import annotations

from collections.abc import Awaitable
from typing import TypedDict

__version__: str

class Hit(TypedDict, total=False):
    """One scored search result from the shared corpus store.

    The first six keys are always present; the provenance keys (``timestamp``
    through ``project``) appear only when the record carries them.
    """

    path: str
    score: float
    start_line: int | None
    num_lines: int | None
    text: str
    source: str
    timestamp: int
    user: str
    host: str
    session_id: str
    external_id: str
    url: str
    repo: str
    project: str

class RerankHit(TypedDict):
    """One reranked text from ``bm25_rerank``: its position in the input batch,
    BM25 score, and body."""

    index: int
    score: float
    text: str

class FileHit(TypedDict):
    """One hit from ``bm25_search`` over an on-disk index."""

    path: str
    score: float
    snippet: str
    chunk_offset: int

class IndexStats(TypedDict):
    """The outcome of a ``bm25_index`` run. ``errors`` pairs each skipped file
    with the reason it could not be indexed."""

    files_indexed: int
    files_skipped: int
    errors: list[list[str]]

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
    reranker: str | None = ...,
    since: int | str | None = ...,
    until: int | str | None = ...,
    compact: bool = ...,
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
    since: int | str | None = ...,
    until: int | str | None = ...,
    compact: bool = ...,
) -> Awaitable[list[Hit]]: ...
def recent(
    top_k: int = ...,
    store: str | None = ...,
    base_url: str | None = ...,
    source: list[str] | None = ...,
    not_source: list[str] | None = ...,
    repo: str | None = ...,
    user: list[str] | None = ...,
    host: list[str] | None = ...,
    project: list[str] | None = ...,
    since: int | str | None = ...,
    until: int | str | None = ...,
    compact: bool = ...,
) -> Awaitable[list[Hit]]: ...

# The BM25 (`file-search`) bindings are synchronous: no network I/O, so they
# return their results directly rather than an awaitable.
def bm25_rerank(
    query: str,
    texts: list[str],
    limit: int | None = ...,
) -> list[RerankHit]: ...
def bm25_index(
    path: str,
    index_dir: str,
    no_gitignore: bool = ...,
) -> IndexStats: ...
def bm25_search(
    query: str,
    index_dir: str,
    limit: int = ...,
    filter: str | None = ...,
) -> list[FileHit]: ...
