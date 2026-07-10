"""``find`` accepts a string ``glob=`` as a path filter, same meaning as on
``grep`` (issue #1366).

Before, ``glob`` was only the bool "the pattern IS a glob" mode flag, so the
grep-parity call ``find("x", glob="*.nix")`` truthily flipped that mode and
silently glob-matched the *pattern* instead of filtering — the exact
cross-helper prior the issue describes. The filter itself (`_glob_filter`) is
pure and exercised directly; the end-to-end path runs against real fd and
skips cleanly when fd is not on PATH.
"""

from __future__ import annotations

import asyncio
from datetime import UTC, datetime

import polars as pl
import pytest

import fsearch


def _frame(paths: list[str]) -> pl.DataFrame:
    """A minimal find-shaped frame for the given paths."""
    now = datetime.now(tz=UTC)
    return pl.DataFrame(
        [
            {"path": p, "name": p.rsplit("/", 1)[-1], "type": "file", "size": 0, "mtime": now}
            for p in paths
        ],
        schema=fsearch._FIND_SCHEMA,
    )


def test_glob_filter_without_slash_matches_the_name() -> None:
    frame = _frame(["/r/a.nix", "/r/sub/c.nix", "/r/b.txt"])
    kept = fsearch._glob_filter(frame, "*.nix", "/r")
    assert kept["path"].to_list() == ["/r/a.nix", "/r/sub/c.nix"]


def test_glob_filter_with_slash_matches_the_root_relative_path() -> None:
    frame = _frame(["/r/a.nix", "/r/sub/c.nix"])
    kept = fsearch._glob_filter(frame, "sub/*.nix", "/r")
    assert kept["path"].to_list() == ["/r/sub/c.nix"]


def test_find_string_glob_filters_instead_of_flipping_glob_mode(tmp_path: object) -> None:
    # The issue's call shape: a name pattern plus a glob filter. A truthy-bool
    # reading would glob-match the pattern "c" (matching nothing) instead of
    # filtering the regex hits down to *.nix.
    import shutil

    if shutil.which("fd") is None:
        pytest.skip("fd not on PATH")
    (tmp_path / "sub").mkdir()  # type: ignore[operator]
    (tmp_path / "a.nix").write_text("x")  # type: ignore[operator]
    (tmp_path / "b.txt").write_text("x")  # type: ignore[operator]
    (tmp_path / "sub" / "c.nix").write_text("x")  # type: ignore[operator]

    frame = asyncio.run(fsearch.find("c", root=str(tmp_path), glob="*.nix"))  # type: ignore[arg-type]
    assert frame["name"].to_list() == ["c.nix"], frame

    everything = asyncio.run(fsearch.find(root=str(tmp_path), glob="*.nix"))  # type: ignore[arg-type]
    assert sorted(everything["name"].to_list()) == ["a.nix", "c.nix"], everything


def test_find_limit_counts_filtered_hits_not_scanned_ones(tmp_path: object) -> None:
    # With a glob filter the source cap (fd --max-results) would count
    # pre-filter hits and could starve the result; limit must apply to rows
    # that survive the filter.
    import shutil

    if shutil.which("fd") is None:
        pytest.skip("fd not on PATH")
    for i in range(10):
        (tmp_path / f"aa{i}.txt").write_text("x")  # type: ignore[operator]
        (tmp_path / f"zz{i}.nix").write_text("x")  # type: ignore[operator]
    frame = asyncio.run(fsearch.find(root=str(tmp_path), glob="*.nix", limit=5))  # type: ignore[arg-type]
    assert frame.height == 5
    assert all(n.endswith(".nix") for n in frame["name"])
