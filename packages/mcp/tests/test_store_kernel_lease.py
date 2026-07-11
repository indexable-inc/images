"""The kernel's board lease: registration facts and the writer's heartbeat.

Weave expires a kernel entity (and its sessions) three missed beats after
its last write (prelude.dl kernel_seen_ms/kernel_expired), so the store --
which rides inside the kernel process -- must (a) register the kernel with
its placement facts (kernel_host from IX_MCP_KERNEL, node, pid) and (b)
keep beating heartbeat_ms while the process lives, WITHOUT the beat
un-idling the agent (no last_active_ms ride-along).
"""

from __future__ import annotations

import asyncio
import hashlib
import threading
import time
from pathlib import Path

import pytest

from ix_notebook_mcp import runtime, store


def _capture(monkeypatch: pytest.MonkeyPatch) -> list[dict]:
    posted: list[dict] = []

    def fake_json(method: str, url: str, *, body: object = None, content: bytes | None = None) -> object:
        if url.endswith("/api/facts"):
            posted.extend(body if isinstance(body, list) else [body])
        return {"seq": 1, "id": "f1"}

    monkeypatch.setattr(store, "_http_json", fake_json)
    return posted


def _facts(posted: list[dict]) -> list[tuple[str, str, object]]:
    out = []
    for item in posted:
        fact = item.get("fact") or {}
        out.append((fact["entity"]["v"], fact["attr"], fact["value"]["v"]))
    return out


def test_registration_carries_placement_and_a_first_beat(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    monkeypatch.setenv("IX_MCP_KERNEL", "ray")
    monkeypatch.delenv("IX_WEAVE_AGENT", raising=False)
    posted = _capture(monkeypatch)

    conn = store.WeaveStore(tmp_path / "s.ixnb")
    try:
        assert conn.flush(timeout=5.0)
    finally:
        conn.close()

    facts = _facts(posted)
    kernel = conn.kernel
    by_attr = {attr: value for entity, attr, value in facts if entity == kernel}
    assert by_attr["type"] == "kernel"
    assert by_attr["kernel_host"] == "ray"
    assert isinstance(by_attr["node"], str)
    assert by_attr["node"]
    assert isinstance(by_attr["heartbeat_ms"], int)
    assert (conn.agent, "on_kernel", kernel) in facts


def test_writer_beats_the_lease_without_unidling_the_agent(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    monkeypatch.delenv("IX_MCP_KERNEL", raising=False)
    posted = _capture(monkeypatch)
    monkeypatch.setattr(store, "_BEAT_S", 0.15)

    conn = store.WeaveStore(tmp_path / "s.ixnb")
    try:
        conn.flush(timeout=5.0)
        registered = len(posted)
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            beats = [f for f in _facts(posted[registered:]) if f[1] == "heartbeat_ms"]
            if len(beats) >= 2:
                break
            time.sleep(0.05)
    finally:
        conn.close()

    tail = _facts(posted[registered:])
    beats = [f for f in tail if f[1] == "heartbeat_ms"]
    assert len(beats) >= 2, f"writer never beat the lease: {tail}"
    assert all(entity == conn.kernel for entity, _, _ in beats)
    # A beat is process liveness, not agent activity: the idle-styling
    # clock (agent last_active_ms) must not advance with it.
    assert all(attr != "last_active_ms" for _, attr, _ in tail), tail


def test_terminal_result_does_not_wait_for_weave_blob_persistence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    monkeypatch.setenv("WEAVE_URL", "http://weave.test")
    code = "land()\nResult.ok('terminal-result')"
    action_landed = threading.Event()
    persistence_blocked = threading.Event()
    release_persistence = threading.Event()
    persistence_failed = threading.Event()
    runner_done = threading.Event()
    summaries: list[dict] = []
    failures: list[BaseException] = []

    def land() -> None:
        action_landed.set()

    def fake_json(
        method: str, url: str, *, body: object = None, content: bytes | None = None
    ) -> object:
        if url.endswith("/api/blob"):
            data = content or b""
            if data != code.encode():
                persistence_blocked.set()
                if not release_persistence.wait(timeout=5.0):
                    raise TimeoutError("test did not release the blocked journal write")
                if not persistence_failed.is_set():
                    persistence_failed.set()
                    raise TimeoutError("weave journal timed out")
            return {"hash": hashlib.sha256(data).hexdigest()}
        if url.endswith("/api/facts"):
            return {"seq": 1, "id": "f1"}
        raise AssertionError(url)

    monkeypatch.setattr(store, "_http_json", fake_json)
    conn = store.connect(tmp_path / "terminal.ixnb")
    assert conn.flush(timeout=2.0)
    monkeypatch.setattr(runtime, "_store", store)
    monkeypatch.setattr(runtime, "_store_conn", conn)
    monkeypatch.setattr(runtime, "_typecheck_enabled", lambda: False)
    monkeypatch.setattr(runtime, "_protected_builtins", {})
    monkeypatch.setattr(runtime, "_baseline_names", frozenset({"land", "Result"}))
    ns = {"land": land, "Result": runtime.Result}
    job = runtime.Job(code, name="issue-2795-regression")
    job._ns = ns

    def run_job() -> None:
        async def run() -> None:
            job.task = asyncio.create_task(runtime._runner(job, ns))
            await job.task
            summaries.append(runtime._job_summary(job))

        # Preserve a worker-thread failure for the main test thread's assertion.
        try:
            asyncio.run(run())
        except BaseException as exc:
            failures.append(exc)
        finally:
            runner_done.set()

    thread = threading.Thread(target=run_job, name="issue-2795-runner")
    thread.start()
    completed_while_blocked = False
    try:
        assert action_landed.wait(timeout=2.0)
        assert persistence_blocked.wait(timeout=2.0)
        completed_while_blocked = runner_done.wait(timeout=1.0)
    finally:
        release_persistence.set()
        thread.join(timeout=5.0)
        conn.close()

    assert not thread.is_alive()
    assert completed_while_blocked, (
        "the terminal result waited behind weave persistence"
    )
    assert failures == []
    assert summaries[0]["status"] == "done"
    assert summaries[0]["running"] is False
    assert summaries[0]["result"] == "terminal-result"
    warning = capsys.readouterr().err
    assert "weave persistence failed" in warning
    assert f"run:{job.id}" in warning
    assert "queued facts will retry" in warning
