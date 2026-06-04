"""Async read-only search over the shared Mixedbread corpus store.

Two query verbs run over the same store the ``indexer`` populates (code plus
agent/shell history across the fleet):

* ``semantic(query)`` runs a natural-language semantic search.
* ``grep(pattern)`` runs a regular expression over the same chunks.

Neither indexes: this is a pure query surface, so importing ``search`` never
uploads the local checkout. Scope a query server-side with any of ``source``,
``not_source``, ``repo``, ``user``, ``host``, ``project``; with no selector the
whole corpus is searched.

    hits = await search.semantic("where is retry backoff configured")
    for hit in hits:
        print(hit["path"], hit["score"])

    # only Claude history, only my records
    hits = await search.semantic("deploy steps", source=["claude_history"], user=["andrew"])

    hits = await search.grep(r"fn \\w+\\(", source=["code"], repo="indexable-inc/index")
    for hit in hits:
        print(hit["path"], hit["text"])

Each awaitable is a native asyncio coroutine bridged from Rust via
pyo3-async-runtimes, so ``await`` it on your own event loop. Each hit is a dict
with keys ``path``, ``score``, ``start_line``, ``num_lines``, ``text``, and
``source`` (e.g. ``code``, ``claude_history``, ``slack``, ``linear``, ``github``, ``web``).
Authentication mirrors the ``search`` CLI: ``MXBAI_API_KEY``, or the token
written by ``mgrep login``.
"""

from __future__ import annotations

from ._search import __version__, grep, semantic

__all__ = ["__version__", "grep", "semantic"]
