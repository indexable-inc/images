"""``grep(files_only=True)`` returns one row per matching *file* (path +
match count) instead of per-line match rows (issue #2246).

Before, listing which files match forced materializing every match row and
deduping paths client-side (``df["path"].unique()`` over the row limit). The
count-line parser (``_count_rows``) is pure and exercised directly — the NUL
path/count separator means a ``:`` in a path cannot corrupt the split, and a
stderr line merged into the stream is dropped; the end-to-end path runs against
real ripgrep and skips cleanly when rg is not on PATH.
"""

from __future__ import annotations

import asyncio
import shutil

import pytest

import fsearch


def test_count_rows_splits_on_nul_not_colon() -> None:
    text = "/r/a:b.txt\x002\n/r/c.txt\x001\n"
    assert fsearch._count_rows(text) == [
        {"path": "/r/a:b.txt", "count": 2},
        {"path": "/r/c.txt", "count": 1},
    ]


def test_count_rows_skips_stderr_noise() -> None:
    text = "rg: /r/denied: Permission denied (os error 13)\n/r/a.txt\x003\n"
    assert fsearch._count_rows(text) == [{"path": "/r/a.txt", "count": 3}]


def test_grep_files_only_counts_matches_per_file(tmp_path: object) -> None:
    if shutil.which("rg") is None:
        pytest.skip("rg not on PATH")
    (tmp_path / "two.txt").write_text("needle needle\n")  # type: ignore[operator]
    (tmp_path / "one.txt").write_text("a needle\n")  # type: ignore[operator]
    (tmp_path / "none.txt").write_text("hay\n")  # type: ignore[operator]

    frame = asyncio.run(fsearch.grep("needle", str(tmp_path), files_only=True))
    assert frame.columns == ["path", "count"]
    got = {row["path"].rsplit("/", 1)[-1]: row["count"] for row in frame.to_dicts()}
    # --count-matches counts individual matches (two.txt has one line, two hits);
    # a file with no match contributes no row.
    assert got == {"two.txt": 2, "one.txt": 1}, frame
    assert not isinstance(frame, fsearch.PartialFrame)


def test_grep_files_only_no_matches_is_an_empty_frame(tmp_path: object) -> None:
    # rg exits 1 on "no matches"; that is a legitimate empty frame, not an error.
    if shutil.which("rg") is None:
        pytest.skip("rg not on PATH")
    (tmp_path / "a.txt").write_text("hay\n")  # type: ignore[operator]
    frame = asyncio.run(fsearch.grep("needle", str(tmp_path), files_only=True))
    assert frame.height == 0
    assert frame.columns == ["path", "count"]


def test_grep_files_only_limit_caps_files_and_flags_partial(tmp_path: object) -> None:
    # In files-only mode `limit` counts files, and a capped result is flagged as
    # partial so a caller cannot mistake it for the complete file list.
    if shutil.which("rg") is None:
        pytest.skip("rg not on PATH")
    for i in range(10):
        (tmp_path / f"f{i}.txt").write_text("needle\n")  # type: ignore[operator]
    frame = asyncio.run(fsearch.grep("needle", str(tmp_path), files_only=True, limit=3))
    assert frame.height == 3
    assert isinstance(frame, fsearch.PartialFrame)
    assert frame.truncated
    assert "limit=3" in frame.reason
