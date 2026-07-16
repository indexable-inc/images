"""Make the package importable when pytest runs from anywhere in the repo."""

import pathlib
import sys

_MCP = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(_MCP))
sys.path.insert(0, str(_MCP / "src" / "weave"))  # store.py imports weave.spool

import pytest


@pytest.fixture
def fake_weave(monkeypatch: pytest.MonkeyPatch) -> object:
    """In-memory Weave ABI double for store.py round-trip tests (hermetic:
    no network, no journal; real-server fidelity pinned by the WEAVE_BIN
    integration test). Returns a weave_stub.FakeWeave."""
    import weave_stub

    return weave_stub.install(monkeypatch)


@pytest.fixture(autouse=True)
def _join_spools(monkeypatch: pytest.MonkeyPatch) -> object:
    """Join every spool flusher thread BEFORE monkeypatched transports revert
    (a live flusher must never fall back to a real weave URL), and reset the
    once-per-URL loud-line latch between tests."""
    from weave import spool

    yield
    spool.close_all()
    with spool._down_lock:
        spool._down_urls.clear()
