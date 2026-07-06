"""``find()`` with an existing directory as the first positional treats it as
the search root (issue #1978).

Listing a directory tree is the most common call, but the first positional is
the pattern and fd rejects any pattern containing a path separator — so the
natural ``find("/abs/dir")`` used to error. The backend is faked so the argv
handed to fd can be asserted directly.
"""

from __future__ import annotations

import asyncio

import pytest

import fsearch


class _Out:
    code = 0
    text = ""


def _capture_sh(monkeypatch: pytest.MonkeyPatch) -> dict[str, list[str]]:
    captured: dict[str, list[str]] = {}

    async def fake_sh(argv: list[str], **_kwargs: object) -> _Out:
        captured["argv"] = argv
        return _Out()

    monkeypatch.setattr(fsearch, "_sh", fake_sh)
    return captured


def _positionals(argv: list[str]) -> list[str]:
    """The pattern/root pair after the ``--`` separator."""
    return argv[argv.index("--") + 1 :]


def test_find_existing_dir_positional_becomes_root(monkeypatch: pytest.MonkeyPatch, tmp_path: object) -> None:
    captured = _capture_sh(monkeypatch)
    asyncio.run(fsearch.find(str(tmp_path)))  # type: ignore[arg-type]
    # fd gets the match-all pattern with the directory as the root.
    assert _positionals(captured["argv"]) == [".", str(tmp_path)]


def test_find_explicit_root_keeps_the_pattern(monkeypatch: pytest.MonkeyPatch, tmp_path: object) -> None:
    # With `root` explicitly given, the first positional stays a pattern —
    # contradictory args are not silently reshuffled (fd rejects the separator).
    captured = _capture_sh(monkeypatch)
    asyncio.run(fsearch.find(str(tmp_path), "/somewhere/else"))  # type: ignore[arg-type]
    assert _positionals(captured["argv"]) == [str(tmp_path), "/somewhere/else"]


def test_dir_root_requires_a_separator_and_an_existing_dir(tmp_path: object) -> None:
    # The reinterpretation gate: a path with a separator naming a real directory
    # qualifies; a bare name (even one shadowing a real directory) or a missing
    # path never does, so name matching is not hijacked.
    assert fsearch._dir_root(str(tmp_path)) == str(tmp_path)
    assert fsearch._dir_root(tmp_path.name) is None  # type: ignore[attr-defined]
    assert fsearch._dir_root(str(tmp_path / "missing")) is None  # type: ignore[operator]
