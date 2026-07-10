"""Make the package importable when pytest runs from anywhere in the repo."""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))

import pytest


@pytest.fixture
def fake_weave(monkeypatch: pytest.MonkeyPatch) -> object:
    """In-memory Weave ABI double for store.py round-trip tests (hermetic:
    no network, no journal; real-server fidelity pinned by the WEAVE_BIN
    integration test). Returns a weave_stub.FakeWeave."""
    import weave_stub

    return weave_stub.install(monkeypatch)
