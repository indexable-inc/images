"""Grow the search size until enough rows survive the client-side filter.

``top_k`` is retrieval depth, not an output count: a client-side filter (a score
threshold, an ``is_in``, anything that does not push to Mixedbread) trims the
fetched window, so a small ``top_k`` can leave very few rows. ``min_results``
asks for an output floor instead: re-search with a growing ``top_k`` until at
least N rows survive the filter, the store is exhausted, or a ceiling is hit.

Kept separate from ``__init__`` (which imports the compiled extension) and pure
(it drives an injected ``fetch`` callable), so the loop's termination logic can
be tested with no built cdylib and no network (see ``tests/test_overfetch.py``).

Mixedbread search has no cursor, so "fetch more" means re-running the search with
a larger ``top_k``; a larger ``top_k`` returns a superset of the smaller one's
ranked hits, so growth is monotonic. The search re-runs each round, so the cost
is geometric in the final size, not linear in the number of rounds.
"""

from __future__ import annotations

from typing import TYPE_CHECKING
from collections.abc import Callable

if TYPE_CHECKING:
    import polars as pl

# Doubling each round, this caps the over-fetch so a filter that matches almost
# nothing cannot grow the search without bound; the loop also stops earlier when
# the store is exhausted (a round returns fewer raw hits than it asked for).
DEFAULT_MAX_TOP_K = 4096


def initial_k(top_k: int, min_results: int | None, max_k: int) -> int:
    """The ``top_k`` for the first search round.

    With no floor, that is just ``top_k``. With a floor, start as deep as the
    floor needs (no point fetching ``top_k`` then immediately growing), but never
    past ``max_k``: ``max_k`` is a hard ceiling on how deep the source will ever
    search, so a ``min_results`` larger than it is capped (raise ``max_k`` to go
    deeper) rather than silently bypassing the cap on the first round.
    """
    if min_results is None:
        return top_k
    return min(max(top_k, min_results), max_k)


def grow_until(
    min_results: int | None,
    start_k: int,
    max_k: int,
    fetch: Callable[[int], tuple[pl.DataFrame, int]],
) -> pl.DataFrame:
    """Return the filtered frame, growing ``top_k`` toward ``min_results`` rows.

    ``fetch(k)`` runs one search at depth ``k`` and returns ``(df, n_raw)``: the
    fully-filtered frame and the number of raw hits the search returned before
    filtering. With ``min_results`` ``None`` this is a single fetch. Otherwise it
    doubles ``k`` until ``df`` has at least ``min_results`` rows, the search is
    exhausted (``n_raw < k``: no deeper hits exist), or ``k`` reaches ``max_k``.
    It can return fewer than ``min_results`` rows in the latter two cases, the
    same way ``head(n)`` returns what exists when the source is shorter than ``n``.
    """
    k = start_k
    df, n_raw = fetch(k)
    if min_results is None:
        return df
    while df.height < min_results and n_raw >= k and k < max_k:
        k = min(k * 2, max_k)
        df, n_raw = fetch(k)
    return df
