"""The kernel's board lease: registration facts and the writer's heartbeat.

Weave expires a kernel entity (and its sessions) three missed beats after
its last write (prelude.dl kernel_seen_ms/kernel_expired), so the store --
which rides inside the kernel process -- must (a) register the kernel with
its placement facts (kernel_host from IX_MCP_KERNEL, node, pid) and (b)
keep beating heartbeat_ms while the process lives, WITHOUT the beat
un-idling the agent (no last_active_ms ride-along).
"""

from __future__ import annotations

import time
from pathlib import Path

import pytest

from ix_notebook_mcp import store


def _capture(monkeypatch: pytest.MonkeyPatch) -> list[dict]:
    posted: list[dict] = []

    def fake_json(method: str, url: str, *, body: object = None, content: bytes | None = None, headers: dict | None = None) -> object:
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
