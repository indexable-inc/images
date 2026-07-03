"""fsearch returns partial results on a timeout instead of discarding them, and
short-circuits a ``limit=`` scan (issue #1754, bug 3).

The timeout path is made deterministic by faking the backend: ``sh`` attaches
whatever the child wrote before the deadline to the ``TimeoutError`` as
``partial_output``, and ``_run`` parses that instead of losing the work. The
``limit`` short-circuit and the ``PartialFrame`` surface are exercised directly.
"""

from __future__ import annotations

import asyncio

import polars as pl
import pytest

import fsearch


def _grep_row(path: str, line_number: int, text: str) -> dict[str, object]:
    """One parsed grep result row, the shape `_stream_rg` yields and `grep`
    wraps into the ``_GREP_SCHEMA`` frame."""
    return {
        "path": path,
        "line_number": line_number,
        "col": 0,
        "match": "needle",
        "line": text,
        "abs_offset": 0,
    }


def test_partial_frame_is_a_dataframe_that_flags_truncation() -> None:
    frame = fsearch.PartialFrame({"a": [1, 2]}, reason="stopped early")
    assert isinstance(frame, pl.DataFrame)
    assert frame.truncated is True
    assert frame.reason == "stopped early"
    assert "partial results: stopped early" in repr(frame)
    # A plain frame has no such attribute, so `getattr(x, "truncated", False)` is
    # a safe truncation check across both.
    assert not hasattr(pl.DataFrame({"a": [1]}), "truncated")


def test_partial_frame_truncation_reaches_the_model_text() -> None:
    # A cell returning a PartialFrame renders through Result.of's NUON path, not
    # repr(), so the truncation note must ride the model text itself -- otherwise
    # a timed-out scan reads as a complete result.
    from ix_notebook_mcp import runtime

    partial = fsearch.PartialFrame({"a": [1]}, reason="stopped early")
    assert "[partial results: stopped early]" in runtime.Result.of(partial).llm_result
    # A plain frame carries no note.
    assert "[partial results" not in runtime.Result.of(pl.DataFrame({"a": [1]})).llm_result


def test_grep_recovers_partial_matches_on_timeout(monkeypatch: pytest.MonkeyPatch) -> None:
    # `grep` streams ripgrep via `_stream_rg`, so the timeout recovery lives there:
    # on a deadline it returns the rows parsed before the kill with `timed_out=True`,
    # and `grep` wraps them in a `PartialFrame`. Fake that return (patching `_sh`
    # would miss the path entirely, since `_stream_rg` calls the subprocess directly).
    rows = [_grep_row(f"f{i}.txt", i + 1, "needle here") for i in range(3)]

    async def fake_stream_rg(*_args: object, **_kwargs: object) -> object:
        return (rows, True, False)  # (rows, timed_out, hit_limit)

    monkeypatch.setattr(fsearch, "_stream_rg", fake_stream_rg)

    frame = asyncio.run(fsearch.grep("needle", "."))
    assert isinstance(frame, fsearch.PartialFrame)
    assert frame.truncated is True
    assert frame.height == 3, frame.height  # the matches found before the deadline
    assert "timed out" in frame.reason


def test_find_recovers_partial_paths_on_timeout(monkeypatch: pytest.MonkeyPatch, tmp_path: object) -> None:
    # fd writes NUL-separated real paths; plant two so lstat resolves them.
    import os

    a = tmp_path / "a.txt"  # type: ignore[operator]
    b = tmp_path / "b.txt"  # type: ignore[operator]
    a.write_text("x")
    b.write_text("y")
    partial = f"{a}\0{b}\0"

    async def fake_sh(*_args: object, **_kwargs: object) -> object:
        exc = TimeoutError("command timed out")
        exc.partial_output = partial  # type: ignore[attr-defined]
        raise exc

    monkeypatch.setattr(fsearch, "_sh", fake_sh)
    frame = asyncio.run(fsearch.find(root=str(tmp_path)))  # type: ignore[arg-type]
    assert isinstance(frame, fsearch.PartialFrame)
    assert frame.height == 2, frame.height
    assert "timed out" in frame.reason
    _ = os  # keep the import used


def test_grep_backend_failure_raises_not_empty_frame(tmp_path: object) -> None:
    # rg exits 2 on a bad pattern (also: unreadable root, invalid glob); that must
    # surface as FsearchError carrying rg's stderr, never as an empty frame
    # indistinguishable from "no matches". Skips cleanly if rg is not on PATH.
    import shutil

    if shutil.which("rg") is None:
        pytest.skip("ripgrep not on PATH")
    with pytest.raises(fsearch.FsearchError, match="exited 2"):
        asyncio.run(fsearch.grep("(", str(tmp_path)))  # type: ignore[arg-type]


def test_grep_short_circuits_at_limit(tmp_path: object) -> None:
    # Real ripgrep: plant many matches, cap at 5, and assert the scan stops there
    # and is flagged partial. Skips cleanly if rg is not on PATH.
    import shutil

    if shutil.which("rg") is None:
        pytest.skip("ripgrep not on PATH")
    for i in range(20):
        (tmp_path / f"f{i}.txt").write_text("needle here\n" * 10)  # type: ignore[operator]
    frame = asyncio.run(fsearch.grep("needle", str(tmp_path), limit=5))  # type: ignore[arg-type]
    assert isinstance(frame, fsearch.PartialFrame)
    assert frame.height == 5
    assert "limit" in frame.reason
