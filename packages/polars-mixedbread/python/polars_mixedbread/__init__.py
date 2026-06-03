"""A Polars IO source backed by Mixedbread store search.

``scan_mixedbread(query, store=...)`` returns a lazy ``pl.LazyFrame`` whose rows
are the hits of one Mixedbread search, with file metadata flattened into typed
columns. It is registered through Polars' official IO-plugin hook
(``register_io_source``), so it composes with the rest of a lazy query and, most
importantly, lets you do in Polars everything Mixedbread does not, above all
``group_by``.

The point is a single, unified API: you filter with ordinary Polars expressions
and the source pushes what it can down to Mixedbread for you.

    import polars as pl
    from polars_mixedbread import scan_mixedbread

    lf = scan_mixedbread("how does retry backoff work", store="index", top_k=500)
    (
        lf.filter(pl.col("source") == "code")  # pushed down to Mixedbread
        .group_by("repo")                       # runs in Polars
        .agg(pl.len(), pl.col("score").mean())
        .sort("len", descending=True)
        .collect()
    )

Predicate pushdown: a ``.filter(...)`` you add is parsed, and the parts that map
to a Mixedbread metadata filter (string ``==``/``!=`` on a metadata column,
combined with ``&``/``|``/``~``) are sent server-side so Mixedbread ranks a
smaller, more relevant set. Anything else (score thresholds, ``is_in``,
substring matches) runs client-side in Polars. The full predicate is always
re-applied locally, so every returned row satisfies it.

Pushdown is not transparent, though, and that is the point of a search source: a
pushed filter is applied by Mixedbread *before* ranking and ``top_k``, so you get
the ``top_k`` best hits *within* the filter. The same predicate written so it
cannot push (e.g. ``pl.col("source").is_in(["code"])`` instead of ``== "code"``)
filters the ``top_k`` *after* ranking, so it can return a different set of rows.
Both are correct (every row matches the predicate); they differ in which
candidates were ranked. Keep filters in pushable form (string ``==``/``!=``) for
the tightest relevance, and raise ``top_k`` if a client-side filter is discarding
too much of the window.

``query`` and ``top_k`` are not predicates: they parameterize the search itself.
``top_k`` is *retrieval depth*, the number of ranked hits the search returns, so
think of the source as "``top_k`` hits for ``query``, as a table". It is not a
final row count:

* a server-pushed filter is applied *before* ``top_k`` (Mixedbread filters, then
  ranks, then returns ``top_k``), so you get up to ``top_k`` of the filtered set;
* a client-side filter is applied *after* ``top_k``, so it trims the table and
  can leave fewer than ``top_k`` rows.

For a final row cap use Polars' own ``.head(n)`` / ``.limit(n)`` (applied last);
``top_k`` only controls how deep the search goes. For an output *floor* instead,
pass ``min_results=N``: when a client-side filter trims the window below N, the
source re-searches with a growing ``top_k`` until at least N rows survive (or the
store is exhausted). Combine ``min_results=N`` with ``.head(N)`` for exactly N
rows out of an arbitrarily selective filter.

Columns: ``text`` (str), ``score`` (f64), ``filename`` (str), ``start_line``
(u32), ``num_lines`` (u32), ``metadata`` (str, the raw JSON), plus one typed
column per entry in ``metadata_columns`` (default: ``source``, ``repo``,
``path``, ``title``, the keys the ``index`` store carries). Point it at another
store by passing ``metadata_columns={...}`` for that store's keys. Only string
columns push down; declare a non-string dtype and that column still filters and
groups, just client-side.

Authentication mirrors the ``search`` surface: ``MXBAI_API_KEY`` if set,
otherwise the token written by ``mgrep login``.

Known limitation: this is top-``k`` retrieval, not a full table scan. A
``group_by`` aggregates only the retrieved window, not the whole store. Raise
``top_k`` (a pushed-down ``filter`` keeps that window relevant) when you need
wider coverage.
"""

from __future__ import annotations

import json
from typing import Iterator

import polars as pl
from polars.io.plugins import register_io_source

from ._overfetch import DEFAULT_MAX_TOP_K, grow_until, initial_k
from ._polars_mixedbread import __version__, search_mixedbread
from ._pushdown import pushdown

__all__ = ["__version__", "scan_mixedbread"]

# Intrinsic columns every search returns, with the dtypes the Rust side produces.
_INTRINSIC: dict[str, pl.DataType] = {
    "text": pl.String(),
    "score": pl.Float64(),
    "filename": pl.String(),
    "start_line": pl.UInt32(),
    "num_lines": pl.UInt32(),
    "metadata": pl.String(),
}

# Metadata keys the shared `index` store carries, surfaced as typed columns by
# default so the headline `filter(pl.col("source") == ...)` works out of the box.
# Override with `metadata_columns=` for a store with different keys.
_DEFAULT_METADATA_COLUMNS: dict[str, pl.DataType] = {
    "source": pl.String(),
    "repo": pl.String(),
    "path": pl.String(),
    "title": pl.String(),
}

def scan_mixedbread(
    query: str,
    *,
    store: str | list[str] = "index",
    top_k: int = 10,
    min_results: int | None = None,
    max_top_k: int = DEFAULT_MAX_TOP_K,
    base_url: str | None = None,
    rerank: bool = True,
    agentic: bool = False,
    score_threshold: float | None = None,
    metadata_columns: dict[str, pl.DataType] | None = None,
) -> pl.LazyFrame:
    """Lazily scan one Mixedbread store search as a Polars ``LazyFrame``.

    ``query`` parameterizes the search; ``top_k`` is the retrieval depth (how
    many ranked hits the source returns, not a final row count, see the module
    docstring). ``store`` is one store name or a list of them (default
    ``"index"``). ``metadata_columns`` maps metadata keys to dtypes to surface as
    typed columns (default: the ``index`` keys).
    ``rerank``/``agentic``/``score_threshold`` tune retrieval.

    ``min_results`` turns ``top_k`` into a floor on the *output*: when a
    client-side filter trims the window below N rows, the source re-searches with
    a growing ``top_k`` until at least N rows survive the filter or the store is
    exhausted. ``max_top_k`` is a hard ceiling on search depth (a ``min_results``
    above it is capped there; raise ``max_top_k`` to go deeper). Leave
    ``min_results`` ``None`` (default) for a single fetch. Combine with
    ``.head(N)`` for exactly N rows.

    Filter with ordinary Polars expressions; string equality on a metadata
    column is pushed to Mixedbread and everything else runs in Polars. See the
    module docstring.
    """
    stores = [store] if isinstance(store, str) else list(store)
    meta_cols = _DEFAULT_METADATA_COLUMNS if metadata_columns is None else metadata_columns
    schema = pl.Schema({**_INTRINSIC, **meta_cols})
    # Only string columns are safe to push down (see `_pushdown.PUSHABLE_OPS`).
    pushable = {name for name, dtype in meta_cols.items() if dtype == pl.String()}

    def _source(
        with_columns: list[str] | None,
        predicate: pl.Expr | None,
        n_rows: int | None,
        batch_size: int | None,  # noqa: ARG001 - single-shot source ignores the hint
    ) -> Iterator[pl.DataFrame]:
        pushed = None if predicate is None else pushdown(predicate, pushable)

        def fetch(k: int) -> tuple[pl.DataFrame, int]:
            columns = search_mixedbread(
                stores,
                query,
                top_k=k,
                base_url=base_url,
                rerank=rerank,
                agentic=agentic,
                score_threshold=score_threshold,
                filters=None if pushed is None else json.dumps(pushed),
            )
            n_raw = len(columns["score"])
            df = pl.DataFrame(columns, schema=_INTRINSIC)
            # Flatten the declared metadata keys out of the JSON column into typed
            # columns. A missing key reads back null; a non-string dtype is cast
            # leniently so a bad value is null rather than an error.
            if meta_cols:
                df = df.with_columns(
                    _metadata_column(name, dtype) for name, dtype in meta_cols.items()
                )
            # The source owns predicate application: Polars does not re-apply it,
            # so this is what makes a partial (or empty) pushdown correct, and it
            # is the filter `min_results` grows the window against.
            if predicate is not None:
                df = df.filter(predicate)
            return df, n_raw

        start_k = initial_k(top_k, min_results, max_top_k)
        df = grow_until(min_results, start_k, max_top_k, fetch)
        if with_columns is not None:
            df = df.select(with_columns)
        if n_rows is not None:
            df = df.head(n_rows)
        yield df

    return register_io_source(_source, schema=schema)


def _metadata_column(name: str, dtype: pl.DataType) -> pl.Expr:
    """Extract metadata key ``name`` from the JSON ``metadata`` column as ``dtype``."""
    expr = pl.col("metadata").str.json_path_match(f"$.{name}")
    if dtype != pl.String():
        expr = expr.cast(dtype, strict=False)
    return expr.alias(name)
