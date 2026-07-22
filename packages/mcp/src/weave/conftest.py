"""Shared pytest fixtures for the weave client tests and the fabric tests
that exercise it. Both files run together in the fabric test derivation
(src/fabric/module.nix), which copies this file in as conftest.py, so the
spool-teardown fixture lives here once instead of in each test module.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest


@pytest.fixture(autouse=True)
def _spool_home(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Iterator[Path]:
    """Every test spools under its own tmp dir; teardown joins flusher
    threads BEFORE monkeypatched transports revert (a live flusher must
    never fall back to a real URL)."""
    from weave import spool as weave_spool

    monkeypatch.setenv("WEAVE_SPOOL", str(tmp_path / "weave-spool"))
    yield tmp_path / "weave-spool"
    weave_spool.close_all()
    weave_spool._down_urls.clear()
