"""A/B micro-benchmark for the scipql binding boundary.

Runs the same workload against whichever `scipql` module is importable, and
prints one JSON document. Drive it twice, once with the hand-written pyo3
build on `sys.path` and once with the unibind-generated build, and compare:

    <old-env>/bin/python3 ab_bench.py --fixture <two-sockets> --label hand
    <new-env>/bin/python3 ab_bench.py --fixture <two-sockets> --label unibind

The workload exercises the boundary, not the engine: `facts` crosses with
thousands of record values per call, `facts_frames` adds the polars wrapping
of the public API, and `rename` (dry-run) returns one string.
"""

from __future__ import annotations

import argparse
import json
import statistics
import time
from collections.abc import Callable


def _time(fn: Callable[[], object], iterations: int) -> dict[str, float]:
    samples: list[float] = []
    for _ in range(iterations):
        start = time.perf_counter()
        fn()
        samples.append(time.perf_counter() - start)
    return {
        "mean_us": statistics.fmean(samples) * 1e6,
        "stdev_us": (statistics.stdev(samples) if len(samples) > 1 else 0.0) * 1e6,
        "min_us": min(samples) * 1e6,
        "iterations": float(iterations),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", required=True, help="two-sockets fixture directory")
    parser.add_argument("--label", required=True, help="which build is under test")
    parser.add_argument("--iterations", type=int, default=300)
    args = parser.parse_args()

    import scipql

    # The public API wraps the extension module; time the raw boundary too.
    from scipql import _scipql

    index_path = args.fixture + "/index.scip"
    raw_facts = _scipql.facts
    # Warm up caches and the first-import costs before sampling.
    for _ in range(20):
        raw_facts(index_path, args.fixture)
        scipql.facts(index_path, args.fixture)
        scipql.rename(index_path, "net/Socket#", "Stream", args.fixture)

    results = {
        "label": args.label,
        "facts_boundary": _time(lambda: raw_facts(index_path, args.fixture), args.iterations),
        "facts_frames": _time(
            lambda: scipql.facts(index_path, args.fixture), args.iterations
        ),
        "rename_dry": _time(
            lambda: scipql.rename(index_path, "net/Socket#", "Stream", args.fixture),
            args.iterations,
        ),
    }
    print(json.dumps(results))


if __name__ == "__main__":
    main()
