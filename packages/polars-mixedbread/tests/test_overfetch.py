"""Offline, deterministic checks for the ``min_results`` over-fetch loop.

Runnable as a plain script (``python tests/test_overfetch.py``) with only Polars
and no built extension. It loads the pure ``_overfetch`` module by path (never
importing the ``polars_mixedbread`` package, which would pull in the compiled
cdylib): pass the module path in ``POLARS_MIXEDBREAD_OVERFETCH`` or argv[1], or
rely on the in-repo default.

The loop must terminate in every case: when enough rows survive, when the store
is exhausted (a round returns fewer raw hits than it asked for), and when the
``max_k`` ceiling is reached. A bug here is either an infinite loop or a silent
under-fetch.
"""

from __future__ import annotations

import importlib.util
import os
import pathlib
import sys
from collections.abc import Callable

import polars as pl

_DEFAULT = pathlib.Path(__file__).resolve().parent.parent / "python" / "polars_mixedbread" / "_overfetch.py"
_explicit = (sys.argv[1] if len(sys.argv) > 1 else None) or os.environ.get("POLARS_MIXEDBREAD_OVERFETCH")
_MODULE_PATH = pathlib.Path(_explicit) if _explicit else _DEFAULT

_spec = importlib.util.spec_from_file_location("polars_mixedbread_overfetch", _MODULE_PATH)
assert _spec is not None, f"cannot load {_MODULE_PATH}"
assert _spec.loader is not None, f"cannot load {_MODULE_PATH}"
_overfetch = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_overfetch)

grow_until = _overfetch.grow_until
initial_k = _overfetch.initial_k


def make_fetch(*, total_hits: int, pass_every: int, calls: list[int]) -> Callable[[int], tuple[pl.DataFrame, int]]:
    """A fake ``fetch(k)``: a store of ``total_hits`` ranked hits where 1 in
    ``pass_every`` survives the client-side filter. Records each ``k`` it sees."""

    def fetch(k: int) -> tuple[pl.DataFrame, int]:
        calls.append(k)
        n_raw = min(k, total_hits)
        survivors = n_raw // pass_every
        return pl.DataFrame({"x": list(range(survivors))}), n_raw

    return fetch


def test_none_is_a_single_fetch() -> None:
    calls: list[int] = []
    df = grow_until(None, start_k=10, max_k=4096, fetch=make_fetch(total_hits=1000, pass_every=1, calls=calls))
    assert calls == [10]
    assert df.height == 10


def test_first_fetch_already_enough() -> None:
    calls: list[int] = []
    # 100% pass rate: start_k=25 yields 25 survivors >= 25.
    df = grow_until(25, start_k=25, max_k=4096, fetch=make_fetch(total_hits=1000, pass_every=1, calls=calls))
    assert calls == [25]
    assert df.height >= 25


def test_grows_until_enough_survive() -> None:
    calls: list[int] = []
    # 10% pass rate: need k>=250 for 25 survivors; doubling from 25 -> 400.
    df = grow_until(25, start_k=25, max_k=4096, fetch=make_fetch(total_hits=10_000, pass_every=10, calls=calls))
    assert calls == [25, 50, 100, 200, 400]
    assert df.height >= 25
    assert calls == sorted(calls)  # monotonic growth


def test_stops_when_store_exhausted() -> None:
    calls: list[int] = []
    # Only 30 hits exist; 10% survive (3), far below min_results=25. The round at
    # k=50 returns n_raw=30 < 50, so the store is exhausted: stop, do not loop.
    df = grow_until(25, start_k=25, max_k=4096, fetch=make_fetch(total_hits=30, pass_every=10, calls=calls))
    assert calls == [25, 50]
    assert df.height < 25  # returns what exists, like head(n) past the end


def test_initial_k() -> None:
    assert initial_k(top_k=10, min_results=None, max_k=4096) == 10  # no floor: just top_k
    assert initial_k(top_k=10, min_results=5, max_k=4096) == 10  # floor below top_k: top_k
    assert initial_k(top_k=10, min_results=50, max_k=4096) == 50  # floor above top_k: start deep
    assert initial_k(top_k=10, min_results=9000, max_k=4096) == 4096  # floor above ceiling: capped


def test_min_results_above_ceiling_is_one_fetch_at_the_cap() -> None:
    # start_k is clamped to max_k by initial_k, so the loop guard `k < max_k` is
    # false immediately: exactly one fetch at the ceiling, never beyond it.
    calls: list[int] = []
    k0 = initial_k(top_k=10, min_results=9000, max_k=100)
    grow_until(9000, start_k=k0, max_k=100, fetch=make_fetch(total_hits=10**9, pass_every=1, calls=calls))
    assert calls == [100]


def test_non_positive_min_results_is_one_fetch() -> None:
    calls: list[int] = []
    df = grow_until(0, start_k=10, max_k=4096, fetch=make_fetch(total_hits=1000, pass_every=10**9, calls=calls))
    assert calls == [10]  # df.height (0) < 0 is false -> no growth
    assert df.height == 0


def test_stops_at_max_k_ceiling() -> None:
    calls: list[int] = []
    # Filter matches nothing; without a ceiling this would grow forever.
    df = grow_until(10, start_k=10, max_k=100, fetch=make_fetch(total_hits=10**9, pass_every=10**9, calls=calls))
    assert calls == [10, 20, 40, 80, 100]
    assert calls[-1] == 100  # clamped to max_k, then stops
    assert df.height == 0


def main() -> None:
    tests = [v for name, v in sorted(globals().items()) if name.startswith("test_") and callable(v)]
    for test in tests:
        test()
    print(f"ok: {len(tests)} overfetch tests passed")


if __name__ == "__main__":
    main()
