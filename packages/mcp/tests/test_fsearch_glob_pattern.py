"""``find("*.py")`` works: a glob-shaped pattern that is not a valid regex
flips to fd's ``--glob`` mode instead of surfacing fd's regex parse error
(issue #2542).

The detector (`_glob_shaped`) is pure and exercised directly; the end-to-end
paths run against real fd and skip cleanly when fd is not on PATH. A pattern
fd still rejects as a regex must at least name the ``glob=``/``fixed=`` escape
hatches in the error.
"""

from __future__ import annotations

import asyncio
import shutil

import pytest

import fsearch


def test_glob_shaped_detects_invalid_regex_with_glob_metachars() -> None:
    assert fsearch._glob_shaped("*.py")
    assert fsearch._glob_shaped("?.py")
    # A valid regex keeps its regex reading, even a glob-plausible one.
    assert not fsearch._glob_shaped(".*py")
    assert not fsearch._glob_shaped("[abc].py")
    # No glob metacharacter: never a glob, even when invalid as a regex.
    assert not fsearch._glob_shaped("(ab")
    assert not fsearch._glob_shaped("src")


def test_find_star_pattern_matches_as_glob(tmp_path: object) -> None:
    # The issue's exact call shape: find("*.py") used to raise
    # "fd exited 1: regex parse error: repetition operator missing expression".
    if shutil.which("fd") is None:
        pytest.skip("fd not on PATH")
    (tmp_path / "a.py").write_text("x")  # type: ignore[operator]
    (tmp_path / "b.txt").write_text("x")  # type: ignore[operator]
    (tmp_path / "sub").mkdir()  # type: ignore[operator]
    (tmp_path / "sub" / "c.py").write_text("x")  # type: ignore[operator]

    frame = asyncio.run(fsearch.find("*.py", root=str(tmp_path)))  # type: ignore[arg-type]
    assert sorted(frame["name"].to_list()) == ["a.py", "c.py"], frame


def test_find_valid_regex_keeps_regex_reading(tmp_path: object) -> None:
    # `.*py` is a valid regex, so autodetect must not hijack it into glob mode
    # (as a glob it would require a literal leading dot and match nothing here).
    if shutil.which("fd") is None:
        pytest.skip("fd not on PATH")
    (tmp_path / "a.py").write_text("x")  # type: ignore[operator]
    (tmp_path / "b.txt").write_text("x")  # type: ignore[operator]

    frame = asyncio.run(fsearch.find(".*py", root=str(tmp_path)))  # type: ignore[arg-type]
    assert frame["name"].to_list() == ["a.py"], frame


def test_find_invalid_regex_error_names_the_escape_hatches(tmp_path: object) -> None:
    # A pattern fd rejects that is NOT glob-shaped still errors, but the error
    # must steer to glob=True / fixed=True instead of a bare fd passthrough.
    if shutil.which("fd") is None:
        pytest.skip("fd not on PATH")
    with pytest.raises(fsearch.FsearchError, match="glob=True"):
        asyncio.run(fsearch.find("(ab", root=str(tmp_path)))  # type: ignore[arg-type]
