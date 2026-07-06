"""In-band kernel build staleness (index#2110).

A stale deploy used to surface only as a confusing call-binding TypeError
("nu() got an unexpected keyword argument 'check'") with no way to tell whether
the API never existed or the running kernel predates it. Two surfaces now carry
the build stamp: the `api()` catalog's header row, and the TypeError hint when
the callable is the kernel's own surface (never a user-defined one).
"""
from __future__ import annotations

import sys
from collections.abc import Callable
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))

import ix_notebook_mcp.runtime as rt


def test_api_first_row_is_build_stamp(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("IX_BUILD_REV", "7e42ccdb18827401226635")
    monkeypatch.setenv("IX_BUILD_EPOCH", "86400")
    frame = rt.api()
    first = frame.row(0, named=True)
    assert first["where"] == "kernel"
    assert first["name"] == "build"
    assert "7e42ccdb1882" in first["sig"]
    assert "1970-01-02" in first["sig"]
    assert "redeploy" in first["summary"]


def test_api_filtered_miss_still_shows_build(monkeypatch: pytest.MonkeyPatch) -> None:
    """The row survives any filter: the staleness signal matters MOST when a
    lookup comes back empty (the helper the docs promised is not deployed)."""
    monkeypatch.setenv("IX_BUILD_REV", "7e42ccdb18827401226635")
    frame = rt.api("no-helper-matches-this-9f3a")
    assert frame.height == 1
    assert frame.row(0, named=True)["name"] == "build"


def _binding_error(fn: Callable[..., object]) -> TypeError:
    try:
        fn(bogus_kwarg_from_the_future=1)
    except TypeError as e:
        return e
    raise AssertionError("expected TypeError")


def kernel_shaped(*, query: str = "") -> None:
    pass


def test_hint_stamps_kernel_surface(monkeypatch: pytest.MonkeyPatch) -> None:
    """A binding error against a registry-known name carries the build stamp,
    so the mismatch is attributable to a stale deploy in the error itself."""
    monkeypatch.setenv("IX_BUILD_REV", "7e42ccdb18827401226635")
    # "grep" is a registry builtin; the namespace object is stand-in, the gate
    # keys on the resolved name.
    exc = TypeError("grep() got an unexpected keyword argument 'bogus'")
    old_ns = rt._user_ns
    try:
        rt._user_ns = {"grep": kernel_shaped}
        hint = rt._type_error_hint(exc)
    finally:
        rt._user_ns = old_ns
    assert "signature" in hint
    assert "Kernel build: 7e42ccdb1882" in hint
    assert "redeploy" in hint


def test_hint_leaves_user_callables_unstamped() -> None:
    """Only the kernel's own surface can be stale relative to an agent's docs;
    a user-defined function keeps the plain signature hint."""
    exc = _binding_error(kernel_shaped)
    old_ns = rt._user_ns
    try:
        rt._user_ns = {"kernel_shaped": kernel_shaped}
        hint = rt._type_error_hint(exc)
    finally:
        rt._user_ns = old_ns
    assert "signature" in hint
    assert "Kernel build" not in hint
