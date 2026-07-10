"""PTY/CAS chunk stream (weave2 n-pty, index write side): bulk job output
never enters the journal as raw facts - it rides CAS with ~1 chunk fact per
flush, while the last_output/line previews stay the cheap derived view.
Hermetic against the weave_stub ABI double."""

from __future__ import annotations

import time

from ix_notebook_mcp import runtime, store


def _connect(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> store.WeaveStore:
    monkeypatch.setenv("IX_WEAVE_AGENT", "agent:test")
    conn = store.connect(tmp_path / "session.ixnb")
    monkeypatch.setattr(runtime, "_store", store)
    monkeypatch.setattr(runtime, "_store_conn", conn)
    return conn


def _wire_facts(fake: object) -> list[dict]:
    return [item["fact"] for item in fake.writes if "fact" in item]


def _stream_facts(fake: object, stream: str, attr: str) -> list[dict]:
    return [f for f in _wire_facts(fake) if f["entity"]["v"] == stream and f["attr"] == attr]


def _chunks(fake: object, stream: str) -> list[bytes]:
    """Chunk payloads for one stream, in journal (write) order."""
    return [fake.blobs[f["value"]["v"]] for f in _stream_facts(fake, stream, "chunk")]


def test_stream_minted_once_and_size_cap_flushes_exact_bytes(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    conn = _connect(tmp_path, monkeypatch)
    job = runtime.Job("", name="j")
    stream = f"pty-stream:{job.id}"
    first = "x" * runtime._STREAM_FLUSH_BYTES  # reaches the size cap: flushes at feed time
    job._append(first)
    job._append("tail")  # below the cap: buffered, not flushed
    assert conn.flush()

    mints = _stream_facts(fake_weave, stream, "type")
    assert [m["value"]["v"] for m in mints] == ["pty_stream"]  # minted exactly once
    assert fake_weave.facts[(stream, "child_of")] == f"run:{job.id}"
    chunk_facts = _stream_facts(fake_weave, stream, "chunk")
    assert [f["value"]["t"] for f in chunk_facts] == ["hash"]  # rides as a typed CAS ref
    assert _chunks(fake_weave, stream) == [first.encode()]  # exactly the flushed range
    # The mint lands before its first chunk (FIFO queue = journal order).
    facts = _wire_facts(fake_weave)
    assert facts.index(mints[0]) < facts.index(chunk_facts[0])
    # The in-memory preview path is untouched by the stream tap.
    assert job.output == first + "tail"
    conn.close()


def test_time_cap_flushes_via_flusher_poll(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    conn = _connect(tmp_path, monkeypatch)
    job = runtime.Job("", name="j")
    stream = f"pty-stream:{job.id}"
    job._append("hello")
    job._stream.poll()  # younger than the cap: nothing flushes
    assert conn.flush()
    assert _chunks(fake_weave, stream) == []
    job._stream._first_ts -= runtime._STREAM_FLUSH_SECS + 0.01  # age the buffer past the cap, no sleeping
    job._stream.poll()
    assert conn.flush()
    assert _chunks(fake_weave, stream) == [b"hello"]
    conn.close()


def test_final_flush_on_exit_and_ranges_reassemble(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    conn = _connect(tmp_path, monkeypatch)
    job = runtime.Job("", name="j")
    stream = f"pty-stream:{job.id}"
    big = "a" * runtime._STREAM_FLUSH_BYTES
    job._append(big)  # size flush 1
    job._append(big)  # size flush 2: identical bytes, same hash, still its own fact
    job._append("the tail")  # buffered until exit
    job.status = "done"
    job.ended = time.time()
    runtime._persist_final(job)  # exit path: stream tail flushes before the finish facts
    assert conn.flush()
    chunks = _chunks(fake_weave, stream)
    # Every flushed range lands as its own chunk fact (the write-behind
    # last-value dedupe must not eat the repeated hash), and concatenating
    # them in journal order reassembles the full output.
    assert chunks == [big.encode(), big.encode(), b"the tail"]
    conn.close()


def test_spawn_stream_hangs_off_process_entity(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    conn = _connect(tmp_path, monkeypatch)
    job = runtime.Job("", name="w", kind="spawn")
    job._append("out")
    job._stream._first_ts -= runtime._STREAM_FLUSH_SECS + 0.01
    job._stream.poll()
    assert conn.flush()
    assert fake_weave.facts[(f"pty-stream:{job.id}", "child_of")] == f"proc:{job.id}"
    conn.close()


def test_snapshot_seam_asserts_snapshot_fact(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, fake_weave: object) -> None:
    conn = _connect(tmp_path, monkeypatch)
    job = runtime.Job("", name="j")
    stream = f"pty-stream:{job.id}"
    state = '{"v":1,"cols":80,"rows":24,"cursor":{"x":0,"y":0},"lines":[{"text":"$ ls"}]}'
    job._stream._snapshot(state)
    assert conn.flush()
    snaps = _stream_facts(fake_weave, stream, "snapshot")
    assert [s["value"]["t"] for s in snaps] == ["hash"]
    assert fake_weave.blobs[snaps[0]["value"]["v"]] == state.encode()
    conn.close()


def test_weave_url_off_is_entirely_inert(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WEAVE_URL", "off")
    monkeypatch.setattr(store, "_WARNED_OFF", True)  # latch is process-global; silence the banner
    conn = store.connect(tmp_path / "off.ixnb")
    monkeypatch.setattr(runtime, "_store", store)
    monkeypatch.setattr(runtime, "_store_conn", conn)
    job = runtime.Job("", name="j")
    job._append("x" * (runtime._STREAM_FLUSH_BYTES * 2))
    job._stream.poll()
    job._stream.close()
    job._stream._snapshot('{"v":1,"cols":80,"rows":24,"cursor":{"x":0,"y":0},"lines":[]}')
    assert job._stream._ent is None  # never minted
    assert job._stream._buf == []  # never even buffered
    with conn._cv:
        assert not conn._queue  # nothing queued at all
    conn.close()
