"""Session files: the store kept across restarts, namespace checkpoints, and the
reopen path (instant restore from the checkpoint plus catch-up replay of cells
that finished after it)."""

from __future__ import annotations

import asyncio

import pytest

from ix_notebook_mcp import runtime, store

# --------------------------------------------------------------------------- #
# store: snapshots, interrupted marking, the replay set
# --------------------------------------------------------------------------- #


def test_snapshot_roundtrip_keeps_only_the_newest(tmp_path) -> None:
    conn = store.connect(tmp_path / "s.ixnb")
    store.save_snapshot(conn, created_at=1.0, blob=b"one", names=["a"], skipped=[])
    store.save_snapshot(
        conn, created_at=2.0, blob=b"two", names=["a", "b"], skipped=[{"name": "s", "reason": "socket"}]
    )
    snap = store.latest_snapshot(conn)
    assert snap["blob"] == b"two"
    assert snap["names"] == ["a", "b"]
    assert snap["skipped"][0]["name"] == "s"
    # Pruned: exactly one checkpoint lives in the file.
    assert conn.execute("SELECT COUNT(*) FROM snapshots").fetchone()[0] == 1


def test_latest_snapshot_is_none_for_a_fresh_file(tmp_path) -> None:
    conn = store.connect(tmp_path / "s.ixnb")
    assert store.latest_snapshot(conn) is None


def test_mark_interrupted_closes_running_rows_and_live_resources(tmp_path) -> None:
    conn = store.connect(tmp_path / "s.ixnb")
    store.start(conn, id="dead", name="dead", code="x", started_at=1.0)
    store.start(conn, id="fine", name="fine", code="y", started_at=1.0)
    store.finish(conn, id="fine", status="done", ended_at=2.0, output="", result=None, error=None)
    store.upsert_resource(
        conn, id="r1", title="t", kind="tui", html="", status="live", created_at=1.0, updated_at=1.0
    )
    assert store.mark_interrupted(conn, ended_at=3.0) == 1
    assert store.get(conn, "dead")["status"] == "interrupted"
    assert store.get(conn, "dead")["ended_at"] == 3.0
    assert store.get(conn, "fine")["status"] == "done"
    assert store.live_resources(conn) == []


def test_replayable_anchors_on_ended_at_and_excludes_replays(tmp_path) -> None:
    conn = store.connect(tmp_path / "s.ixnb")
    # Finished before the checkpoint: captured by it, not replayed.
    store.start(conn, id="old", name="old", code="a = 1", started_at=1.0)
    store.finish(conn, id="old", status="done", ended_at=2.0, output="", result=None, error=None)
    # Started before but FINISHED after the checkpoint: partial effects in the
    # checkpoint, so it must replay.
    store.start(conn, id="straddle", name="straddle", code="b = 2", started_at=1.5)
    store.finish(conn, id="straddle", status="done", ended_at=6.0, output="", result=None, error=None)
    # Finished after the checkpoint: replays.
    store.start(conn, id="new", name="new", code="c = 3", started_at=7.0)
    store.finish(conn, id="new", status="done", ended_at=8.0, output="", result=None, error=None)
    # Failed and replay-kind rows never replay.
    store.start(conn, id="bad", name="bad", code="boom", started_at=7.0)
    store.finish(conn, id="bad", status="error", ended_at=8.0, output="", result=None, error="x")
    store.start(conn, id="rep", name="rep", code="d = 4", started_at=7.0, kind="replay")
    store.finish(conn, id="rep", status="done", ended_at=8.0, output="", result=None, error=None)

    assert [r["id"] for r in store.replayable(conn, since=5.0)] == ["straddle", "new"]
    # No checkpoint at all: the whole successful original log, oldest first.
    assert [r["id"] for r in store.replayable(conn, since=None)] == ["old", "straddle", "new"]


def test_kind_column_round_trips(tmp_path) -> None:
    conn = store.connect(tmp_path / "s.ixnb")
    store.start(conn, id="r", name="r", code="x", started_at=1.0, kind="replay")
    assert store.get(conn, "r")["kind"] == "replay"


# --------------------------------------------------------------------------- #
# runtime: checkpoint payloads
# --------------------------------------------------------------------------- #


def test_snapshot_candidates_filter(monkeypatch) -> None:
    import types as types_mod

    ns = {
        "keep": 41,
        # A single-underscore USER name is real state and must be checkpointed
        # (only dunders and IPython's history machinery are kernel-internal).
        "_cfg": 1,
        "__dunder": 2,
        "baseline": 3,
        "module": types_mod,
        # IPython's lazily-created machinery (not in the baseline): result and
        # input caches, history dicts.
        "_": 4,
        "__": 5,
        "_i": "code",
        "_ii": "code",
        "_i7": "code",
        "_7": 6,
        "_oh": {},
        "_ih": [],
        "_dh": [],
        "_exit_code": 0,
    }
    monkeypatch.setattr(runtime, "_baseline_names", frozenset({"baseline"}))
    assert set(runtime._snapshot_candidates(ns)) == {"keep", "_cfg"}


def test_snapshot_payload_skips_the_unpicklable() -> None:
    blob, names, skipped = runtime._snapshot_payload({"ok": [1, 2], "bad": (i for i in ())})
    assert names == ["ok"]
    assert skipped[0]["name"] == "bad"
    import pickle

    named = pickle.loads(blob)
    assert set(named) == {"ok"}


# --------------------------------------------------------------------------- #
# the reopen path, end to end and in-process: run cells against a session store,
# checkpoint, "restart" into a fresh namespace, restore.
# --------------------------------------------------------------------------- #


def _wire(monkeypatch, conn, ns) -> None:
    monkeypatch.setattr(runtime, "_store", store)
    monkeypatch.setattr(runtime, "_store_conn", conn)
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_SESSION", True)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))


def test_session_reopen_restores_instantly_and_replays_the_gap(tmp_path, monkeypatch) -> None:
    pytest.importorskip("dill")
    path = tmp_path / "s.ixnb"

    async def first_run() -> None:
        conn = store.connect(path)
        ns = {"Result": runtime.Result}
        _wire(monkeypatch, conn, ns)
        # _cfg is the review-confirmed loss window: an underscore name bound in
        # a cell that is captured by the checkpoint (so replay never re-runs it)
        # must come back from the checkpoint itself.
        await runtime.__ix_run("x = 40\n_cfg = 'v1'\ndef double(n):\n    return n * 2\nResult.ok('a')\n")
        await runtime._snapshot_now()
        # A cell that finishes AFTER the checkpoint: only covered by replay.
        await runtime.__ix_run("y = double(x) + 4\nResult.ok('b')\n")
        conn.close()

    asyncio.run(first_run())

    async def reopen() -> dict:
        conn = store.connect(path)
        ns = {"Result": runtime.Result}
        _wire(monkeypatch, conn, ns)
        monkeypatch.setattr(runtime, "jobs", {})
        await runtime.__ix_restore()
        conn.close()
        return ns

    ns = asyncio.run(reopen())
    # x, _cfg, and double came back from the checkpoint; y from replaying the
    # last cell (which itself needs the restored names to evaluate).
    assert ns["x"] == 40
    assert ns["_cfg"] == "v1"
    assert ns["double"](3) == 6
    assert ns["y"] == 84

    # The restore folded everything into a fresh checkpoint, so a SECOND reopen
    # replays nothing (replay rows are excluded; originals predate the new
    # checkpoint).
    conn = store.connect(path)
    snap = store.latest_snapshot(conn)
    assert snap is not None and {"x", "y", "double"} <= set(snap["names"])
    assert store.replayable(conn, since=snap["created_at"]) == []
    conn.close()


def test_restore_without_checkpoint_replays_the_full_log(tmp_path, monkeypatch) -> None:
    path = tmp_path / "s.ixnb"

    async def first_run() -> None:
        conn = store.connect(path)
        ns = {"Result": runtime.Result}
        _wire(monkeypatch, conn, ns)
        monkeypatch.setattr(runtime, "_SESSION", False)  # no checkpointing at all
        await runtime.__ix_run("x = 1\nResult.ok('a')\n")
        await runtime.__ix_run("x = x + 1\nResult.ok('b')\n")
        conn.close()

    asyncio.run(first_run())

    async def reopen() -> dict:
        conn = store.connect(path)
        ns = {"Result": runtime.Result}
        _wire(monkeypatch, conn, ns)
        monkeypatch.setattr(runtime, "_SESSION", False)
        monkeypatch.setattr(runtime, "jobs", {})
        await runtime.__ix_restore()
        conn.close()
        return ns

    assert asyncio.run(reopen())["x"] == 2
