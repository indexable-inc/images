"""Async code search over a content-addressed Mixedbread index.

Two query verbs run over the same indexed, deduplicated chunks:

* ``semantic(query, path)`` runs a natural-language semantic search.
* ``grep(pattern, path)`` runs a regular expression over the same chunks.

Both index the checkout at ``path`` first (uploading only new file content,
deduplicated across worktrees) and return the hits scoped to it. The whole
indexing and query pipeline lives in the Rust ``search-core`` crate;
this package is a thin PyO3 binding over it.

    hits = await search.semantic("where is retry backoff configured", ".")
    for hit in hits:
        print(hit["path"], hit["score"])

    hits = await search.grep(r"fn \\w+\\(", ".", case_sensitive=True)
    for hit in hits:
        print(hit["path"], hit["text"])

Each awaitable is a native asyncio coroutine bridged from Rust via
pyo3-async-runtimes, so ``await`` it on your own event loop. Each hit is a dict
with keys ``path``, ``score``, ``start_line``, ``num_lines``, ``text``, and
``is_web``. Authentication mirrors the ``search`` CLI: ``MXBAI_API_KEY``,
or the token written by ``mgrep login``.
"""

from __future__ import annotations

from ._search import __version__, grep, semantic

__all__ = ["__version__", "grep", "semantic"]
