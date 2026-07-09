"""Integration against a REAL weave server (docs/weave2.md Phase 0).

Gated on WEAVE_BIN (path to a `weave` binary from indexable-inc/weave with
the weave2 schema seeds). Skipped when absent so unit CI stays hermetic;
run locally with e.g.:

    WEAVE_BIN=~/Documents/Git/indexable/weave/target/debug/weave \
        uv run pytest packages/mcp/tests/test_weave_integration.py -q

Pins the fidelity the in-memory stub (weave_stub.py) cannot: real ApiValue
wire shapes, real datalog derivation, and the Phase 0 success criterion
`?- visible(A), weight(A, W).`
"""

from __future__ import annotations

import asyncio
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

import pytest

WEAVE_BIN = os.environ.get("WEAVE_BIN", "")

pytestmark = pytest.mark.skipif(not WEAVE_BIN, reason="WEAVE_BIN not set")


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


@pytest.fixture
def weave_server(tmp_path: Path):
    port = _free_port()
    store_dir = tmp_path / "weave-store"
    subprocess.run([WEAVE_BIN, "--store", str(store_dir), "init"], check=True, capture_output=True)
    proc = subprocess.Popen(
        [WEAVE_BIN, "--store", str(store_dir), "serve", "--addr", f"127.0.0.1:{port}"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    url = f"http://127.0.0.1:{port}"
    try:
        from urllib.request import urlopen

        for _ in range(100):
            try:
                urlopen(f"{url}/api/info", timeout=1).read()
                break
            except Exception:
                time.sleep(0.1)
        else:
            raise RuntimeError("weave serve never came up")
        yield url
    finally:
        proc.terminate()
        proc.wait(timeout=10)


def test_phase0_store_roundtrip_against_real_weave(weave_server, tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("WEAVE_URL", weave_server)
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:e2e")
    from ix_notebook_mcp import store

    now = time.time()
    ws = store.connect(tmp_path / "session.ixnb")
    store.set_session(ws, name="e2e demo session", client="e2e-harness")
    store.start(ws, id="run-1", name="smoke import", code="print('hello weave')",
                started_at=now, budget=15.0, kind="cell", topic="swap-e2e")
    store.update_output(ws, "run-1", "hello weave\n", line=1)
    store.finish(ws, id="run-1", status="done", ended_at=now + 2.5,
                 output="hello weave\n", result="'ok'", error=None,
                 outputs=[], bindings={}, namespace=[])
    store.start(ws, id="job-1", name="background watcher", code="watch()",
                started_at=now, budget=0.0, kind="spawn", topic="swap-e2e")
    store.save_snapshot(ws, created_at=now, blob=b"snapshot-bytes" * 10,
                        names=["x", "y"], skipped=[])
    assert ws.flush(timeout=15.0), "write queue failed to drain"

    got = {r["id"]: r for r in store.recent(ws, limit=10)}
    assert got["run-1"]["status"] == "done"
    assert got["run-1"]["code"] == "print('hello weave')"
    assert store.get_session(ws)["name"] == "e2e demo session"
    snap = store.latest_snapshot(ws)
    assert snap and snap["blob"] == b"snapshot-bytes" * 10
    assert snap["names"] == ["x", "y"]
    assert any(r["id"] == "run-1" for r in store.replayable(ws, None))

    # Phase 0 success criterion, straight datalog against the server.
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src" / "weave"))
    import weave as weave_mod

    async def check() -> None:
        out = await weave_mod.query("?- visible(A), weight(A, W).")
        assert ["agent:e2e", 1] in [list(r) for r in out["rows"]], out["rows"]
        kinds = {tuple(r) for r in (await weave_mod.query('?- child_of(R, "agent:e2e"), type(R, T).'))["rows"]}
        assert ("run:run-1", "run") in kinds
        assert ("proc:job-1", "process") in kinds

    asyncio.run(check())


def test_supervisor_spawn_and_reply_loop(weave_server, tmp_path, monkeypatch) -> None:
    fake = tmp_path / "fake-harness.sh"
    fake.write_text("#!/bin/sh\necho \"fake harness ran: $1\"\n")
    fake.chmod(0o755)
    monkeypatch.setenv("WEAVE_URL", weave_server)
    monkeypatch.setenv("IX_WEAVE_HARNESS_BIN", str(fake))
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src" / "weave"))
    import weave
    from weave import supervisor

    async def main() -> None:
        req = await weave.spawn("prefab:claude-worker", task="say hello", requested_by="agent:main")
        assert any(r[0] == req for r in (await weave.query("?- open_spawn_request(R, P)."))["rows"])
        sup = asyncio.create_task(supervisor.run(answer_main=False))
        try:
            agent = None
            for _ in range(60):
                rows = (await weave.query(f'?- fact(A, "fulfills", "{req}").'))["rows"]
                if rows:
                    agent = rows[0][0]
                    break
                await asyncio.sleep(0.5)
            assert agent, "spawn request never fulfilled"
            attrs = {r[0]: r[1] for r in (await weave.query(f'?- latest("{agent}", A, V).'))["rows"]}
            assert attrs.get("spawned_by") == "agent:main"
            assert not any(
                r[0] == req for r in (await weave.query("?- open_spawn_request(R, P)."))["rows"]
            )
            await weave.chat("are you alive?", to=agent, author="hari")
            for _ in range(60):
                rows = (await weave.query(f'?- from(M, "{agent}"), text(M, T).'))["rows"]
                if rows:
                    assert "fake harness ran" in rows[0][1]
                    return
                await asyncio.sleep(0.5)
            raise AssertionError("agent never replied to the direct message")
        finally:
            sup.cancel()

    asyncio.run(main())
