"""Async read-only search over the shared Mixedbread corpus store.

Three query verbs run over the same store the ``indexer`` populates (code plus
agent/shell history across the fleet):

* ``semantic(query)`` runs a natural-language semantic search.
* ``grep(pattern)`` runs a regular expression over the same chunks.
* ``recent()`` lists the newest records (descending timestamp), no semantic
  scoring -- a deterministic "what happened lately" feed.

Each is a coroutine returning a :class:`polars.DataFrame` (one row per hit), so
``await`` it and compose ``.filter`` / ``.group_by`` / ``.sort`` / ``.head`` on
the result, exactly like ``fff`` and ``view``::

    df = await search.semantic("where is retry backoff configured")
    df.select("path", "score", "timestamp").head()
    df.group_by("source").len().sort("len", descending=True)

None of them indexes: this is a pure query surface, so importing ``search``
never uploads the local checkout. Scope a query server-side with any of
``source``, ``not_source``, ``repo``, ``user``, ``host``, ``project``, and a
time window ``since``/``until`` (epoch seconds or relative spans like
``"24h"``/``"7d"``); with no selector the whole corpus is searched.

    # only Claude history, only my records, last two weeks, token-frugal
    df = await search.semantic(
        "deploy steps", source=["claude_history"], user=["andrew"],
        since="2w", compact=True,
    )

    # my shell commands of the last six hours, newest first
    df = await search.recent(source=["shell"], user=["andrew"], since="6h")

    df = await search.grep(r"fn \\w+\\(", source=["code"], repo="indexable-inc/index")

Every frame has the same stable schema (so ``df["timestamp"]`` and
``df.group_by("source")`` work even on an empty result): the six always-present
columns ``path``, ``score``, ``start_line``, ``num_lines``, ``text``,
``source``, then provenance columns ``timestamp`` (epoch seconds), ``user``,
``host``, ``session_id``, ``external_id``, ``url``, ``repo``, ``project`` --
null where a record does not carry them. Valid sources: ``claude_history``,
``codex``, ``shell``, ``claude_debug``, ``git``, ``github``, ``slack``,
``linear``, ``code``, ``web`` -- an unknown source raises ``ValueError``
instead of silently returning zero hits.

``compact=True`` collapses repeated chunks of one document and caps snippets
at 400 chars (a default ``top_k=10`` full response measured ~20k tokens).
``agentic`` defaults to ``False`` everywhere: it costs 10-23s and ~5x the
per-query price, and may return fewer than ``top_k`` hits.

Authentication mirrors the ``search`` CLI: ``MXBAI_API_KEY``, or the token
written by ``mgrep login``.

Three BM25 verbs, backed by the local ``file-search`` (Tantivy) engine, sit
alongside the corpus queries. They do no network I/O, so unlike the three
above they are plain synchronous functions returning lists/dicts (no
``await``, no DataFrame):

* ``bm25_rerank(query, texts, limit=None)`` reranks a batch of in-memory
  strings, returning dicts ``{index, score, text}``.
* ``bm25_index(path, index_dir)`` builds/updates an on-disk index.
* ``bm25_search(query, index_dir, limit=10, filter=None)`` searches it,
  returning dicts ``{path, score, snippet, chunk_offset}``.

All three inherit the shared ``code-tokenizer`` (camelCase / snake_case /
kebab-case / whitespace splitting plus stemming), so ``"widget factory"``
matches both ``makeWidgetFactory`` and ``make_widget_factory``::

    hits = search.bm25_rerank("retry backoff", candidate_texts, limit=5)
"""

from __future__ import annotations

import functools
from typing import TYPE_CHECKING

from ._search import __version__
from ._search import bm25_index, bm25_rerank, bm25_search
from ._search import grep as _grep
from ._search import recent as _recent
from ._search import semantic as _semantic

if TYPE_CHECKING:
    from collections.abc import Awaitable, Callable

    import polars as pl

    from ._search import Hit

    # A polars dtype as the schema/`cast` APIs accept it: the class (`pl.Utf8`)
    # or an instance. The `_dtypes` table holds the classes.
    _DType = type[pl.DataType] | pl.DataType
    # The native bindings resolve to a list of `Hit` dicts (see `_search.pyi`);
    # the wrapper frames them. Awaiting the framed function yields a DataFrame.
    _Raw = Callable[..., Awaitable[list[Hit]]]
    _Framed = Callable[..., Awaitable[pl.DataFrame]]

__all__ = [
    "__version__",
    "bm25_index",
    "bm25_rerank",
    "bm25_search",
    "grep",
    "recent",
    "semantic",
]

# The stable column set, in display order: the six fields every hit carries,
# then the provenance fields a record may or may not carry. Enforced on every
# result so callers can rely on the schema even when the result is empty or a
# provenance key is absent from every row.
_COLUMNS: tuple[str, ...] = (
    "path",
    "score",
    "start_line",
    "num_lines",
    "text",
    "source",
    "timestamp",
    "user",
    "host",
    "session_id",
    "external_id",
    "url",
    "repo",
    "project",
)


def _dtypes() -> dict[str, _DType]:
    import polars as pl

    return {
        "path": pl.Utf8,
        "score": pl.Float64,
        "start_line": pl.Int64,
        "num_lines": pl.Int64,
        "text": pl.Utf8,
        "source": pl.Utf8,
        "timestamp": pl.Int64,
        "user": pl.Utf8,
        "host": pl.Utf8,
        "session_id": pl.Utf8,
        "external_id": pl.Utf8,
        "url": pl.Utf8,
        "repo": pl.Utf8,
        "project": pl.Utf8,
    }


def _to_frame(rows: list[Hit]) -> pl.DataFrame:
    try:
        import polars as pl
    except ModuleNotFoundError as exc:  # pragma: no cover - env always has polars
        raise ModuleNotFoundError(
            "search.semantic/grep/recent return polars DataFrames; "
            "install polars to use them."
        ) from exc

    dtypes = _dtypes()
    if not rows:
        return pl.DataFrame(schema=dtypes)
    df = pl.DataFrame(rows)
    missing = [c for c in _COLUMNS if c not in df.columns]
    if missing:
        df = df.with_columns([pl.lit(None).cast(dtypes[c]).alias(c) for c in missing])
    df = df.with_columns([pl.col(c).cast(dtypes[c]) for c in _COLUMNS])
    # Standard columns first, in order; any unexpected key kept after them so a
    # new provenance field is surfaced rather than silently dropped.
    extra = [c for c in df.columns if c not in _COLUMNS]
    return df.select([*_COLUMNS, *extra])


def _with_framed_doc(fn: _Framed, doc: str) -> _Framed:
    """Set ``fn.__doc__`` and return it.

    Assigning ``wrapper.__doc__`` directly inside ``_framed`` trips strict type
    checkers: a ``functools.wraps``-decorated local with no docstring is inferred
    to have ``__doc__: None``, so a later ``str`` assignment is rejected. Routing
    it through a plain ``Callable`` (whose ``__doc__`` is the normal
    ``str | None``) keeps the assignment honest without a blanket ignore.
    """
    fn.__doc__ = doc
    return fn


def _framed(raw: _Raw) -> _Framed:
    """Wrap a native PyO3 coroutine so awaiting it yields a polars DataFrame.

    ``functools.wraps`` copies the binding's signature and docstring, so
    ``help()`` and ``inspect.signature`` show the real parameters while
    ``inspect.iscoroutinefunction`` reports ``True``.
    """

    @functools.wraps(raw)
    async def wrapper(*args: object, **kwargs: object) -> pl.DataFrame:
        return _to_frame(await raw(*args, **kwargs))

    return _with_framed_doc(
        wrapper,
        (raw.__doc__ or "")
        + "\n\nReturns a polars DataFrame (one row per hit) with the stable "
        "schema documented on the `search` module.",
    )


semantic = _framed(_semantic)
grep = _framed(_grep)
recent = _framed(_recent)
