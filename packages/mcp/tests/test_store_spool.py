"""Durable-local-first store writes (index#3418, index#3419): every fact
lands in an fsync'd spool next to the store path before the call returns,
store ops never block on the weave wire, the flusher drains in append order
once weave is reachable, and an outage prints exactly one loud line.
Hermetic against the weave_stub ABI double."""

from __future__ import annotations

import json
import threading
import time
from pathlib import Path

import pytest

import weave_stub

from ix_notebook_mcp import store
from weave import spool


@pytest.fixture(autouse=True)
def _joined_spools() -> object:
    """Join flusher threads BEFORE monkeypatches revert and reset the
    once-per-URL loud-line latch (self-contained: this file also runs in the
    nix smoke sandbox where tests/conftest.py is not copied in)."""
    with spool._down_lock:
        spool._down_urls.clear()
    yield
    spool.close_all()


def _spool_lines(conn: store.WeaveStore) -> list[dict]:
    lines: list[dict] = []
    for seg in sorted(Path(f"{conn.path}.spool").glob("w-*.jsonl")):
        lines.extend(json.loads(line) for line in seg.read_text().splitlines() if line)
    return lines


def _refs(lines: list[dict]) -> list[tuple[str, str]]:
    """(entity, attr) per spooled line, blob refs expanded in place: the
    exact order the flusher must deliver."""
    out: list[tuple[str, str]] = []
    for item in lines:
        if "fact" in item:
            out.append((item["fact"]["entity"]["v"], item["fact"]["attr"]))
        else:
            out.extend((ref["entity"]["v"], ref["attr"]) for ref in item["refs"])
    return out


def _gate(monkeypatch: pytest.MonkeyPatch, fake: object) -> dict:
    """Wrap the stub transport so the wire can be taken down per-test."""
    down = {"is": True}
    real = fake.http_json

    def gated(method: str, url: str, *, body: object = None, content: bytes | None = None, headers: dict | None = None) -> object:
        if down["is"]:
            raise ConnectionError("weave down")
        return real(method, url, body=body, content=content, headers=headers)

    monkeypatch.setattr(store, "_http_json", gated)
    return down


def test_outage_writes_stay_durable_then_drain_in_order(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
    fake_weave = weave_stub.install(monkeypatch)
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:test")
    down = _gate(monkeypatch, fake_weave)
    conn = store.connect(tmp_path / "session.ixnb")
    store.start(conn, id="abc", name="Example", code="x = 1", started_at=10.0)
    store.finish(conn, id="abc", kind="cell", status="done", ended_at=12.0, output="hi", result="1", error=None, outputs=[{"text": "hi"}], bindings={"x": 1}, namespace=[])
    assert conn.flush(timeout=1.0) is False  # weave down: pending, not dropped

    lines = _spool_lines(conn)
    expected = _refs(lines)
    assert ("run:abc", "status") in expected
    assert any(attr == "code" for _entity, attr in expected)
    assert fake_weave.writes == []  # nothing reached the wire yet
    # Exactly one loud line per outage, not one per write.
    assert capsys.readouterr().err.count("unreachable") == 1

    down["is"] = False
    assert conn.flush(timeout=10.0)
    assert "reachable again" in capsys.readouterr().err
    got = [(item["fact"]["entity"]["v"], item["fact"]["attr"]) for item in fake_weave.writes]
    assert got == expected  # delivered exactly in local append order
    code = [f for f in (item["fact"] for item in fake_weave.writes) if f["entity"]["v"] == "run:abc" and f["attr"] == "code"]
    assert code[0]["value"]["t"] == "hash"
    assert fake_weave.blobs[code[0]["value"]["v"]] == b"x = 1"  # deferred CAS put resolved at drain
    conn.close()


def test_slow_weave_never_blocks_store_ops(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """The pre-spool failure mode: a slow weave wedging the kernel because
    store writes ran synchronously on the caller's thread."""
    fake_weave = weave_stub.install(monkeypatch)
    release = threading.Event()
    real = fake_weave.http_json

    def slow(method: str, url: str, *, body: object = None, content: bytes | None = None, headers: dict | None = None) -> object:
        assert release.wait(timeout=30), "test bug: gate never released"
        return real(method, url, body=body, content=content, headers=headers)

    monkeypatch.setattr(store, "_http_json", slow)
    started = time.monotonic()
    conn = store.connect(tmp_path / "session.ixnb")  # mints facts: must not wait on the wire
    store.start(conn, id="abc", name="Example", code="x = 1", started_at=10.0)
    store.save_snapshot(conn, created_at=11.0, blob=b"state", names=["x"], skipped=[])
    assert time.monotonic() - started < 5.0
    release.set()
    assert conn.flush(timeout=10.0)
    assert fake_weave.facts[("run:abc", "type")] == "run"
    conn.close()
