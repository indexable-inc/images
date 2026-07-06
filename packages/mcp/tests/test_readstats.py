"""Redundant-read tracking (index#1924).

Every kernel path that reads a real file into agent context records the read; a
read is redundant when the same (path, content) pair was seen earlier in the same
session. These tests pin the tracker's core contract -- same-content re-read
counts as redundant, changed content does not, a different path does not -- plus
the exact ``mcp_read_stats`` journald line the ix fleet pipeline parses.
"""

from __future__ import annotations

import json
import pathlib

from ix_notebook_mcp import readstats


def test_same_content_reread_is_redundant(tmp_path: pathlib.Path) -> None:
    p = tmp_path / "a.py"
    text = "x = 1\n"
    tracker = readstats.ReadStatsTracker()

    assert tracker.record("s1", p, text) is False  # first read: novel
    assert tracker.record("s1", p, text) is True  # byte-identical re-read: redundant

    snap = tracker.snapshot("s1")
    assert snap == {"total_reads": 2, "redundant_reads": 1}


def test_changed_content_is_not_redundant(tmp_path: pathlib.Path) -> None:
    p = tmp_path / "a.py"
    tracker = readstats.ReadStatsTracker()

    assert tracker.record("s1", p, "x = 1\n") is False
    assert tracker.record("s1", p, "x = 2\n") is False  # same path, new bytes

    assert tracker.snapshot("s1") == {"total_reads": 2, "redundant_reads": 0}


def test_same_content_different_path_is_not_redundant(tmp_path: pathlib.Path) -> None:
    text = "shared\n"
    a = tmp_path / "a.py"
    b = tmp_path / "b.py"
    tracker = readstats.ReadStatsTracker()

    assert tracker.record("s1", a, text) is False
    assert tracker.record("s1", b, text) is False  # same bytes, different file

    assert tracker.snapshot("s1") == {"total_reads": 2, "redundant_reads": 0}


def test_counters_are_per_session(tmp_path: pathlib.Path) -> None:
    p = tmp_path / "a.py"
    text = "x = 1\n"
    tracker = readstats.ReadStatsTracker()

    tracker.record("s1", p, text)
    # A re-read in a DIFFERENT session is novel there: each session tracks its own
    # seen-set, so this is not redundant.
    assert tracker.record("s2", p, text) is False
    assert tracker.snapshot("s1") == {"total_reads": 1, "redundant_reads": 0}
    assert tracker.snapshot("s2") == {"total_reads": 1, "redundant_reads": 0}


def test_none_session_maps_to_shared_key(tmp_path: pathlib.Path) -> None:
    p = tmp_path / "a.py"
    text = "x = 1\n"
    tracker = readstats.ReadStatsTracker()

    tracker.record(None, p, text)
    assert tracker.record(None, p, text) is True  # shared session tracks redundancy
    assert tracker.snapshot(None) == {"total_reads": 2, "redundant_reads": 1}


def test_bytes_and_text_hash_identically(tmp_path: pathlib.Path) -> None:
    p = tmp_path / "a.py"
    tracker = readstats.ReadStatsTracker()

    assert tracker.record("s1", p, "café\n") is False
    # The bytes form of the same content is the same read (UTF-8), so redundant.
    assert tracker.record("s1", p, "café\n".encode()) is True


def test_emit_line_matches_contract(tmp_path: pathlib.Path, capfd) -> None:  # noqa: ANN001 -- pytest fixture
    p = tmp_path / "a.py"
    text = "x = 1\n"
    tracker = readstats.ReadStatsTracker()
    tracker.record("sess-42", p, text)
    tracker.record("sess-42", p, text)

    tracker.emit_changed()
    line = capfd.readouterr().err.strip()

    # Exact key order and shape the ix fleet pipeline (ix#6453) parses.
    assert line == (
        '{"event":"mcp_read_stats","session":"sess-42",'
        '"total_reads":2,"redundant_reads":1,"window_s":300}'
    )
    # ...and it is valid JSON with the documented fields.
    assert json.loads(line) == {
        "event": "mcp_read_stats",
        "session": "sess-42",
        "total_reads": 2,
        "redundant_reads": 1,
        "window_s": 300,
    }


def test_emit_changed_only_speaks_when_counts_change(tmp_path: pathlib.Path, capfd) -> None:  # noqa: ANN001 -- pytest fixture
    p = tmp_path / "a.py"
    tracker = readstats.ReadStatsTracker()
    tracker.record("s1", p, "x = 1\n")

    tracker.emit_changed()
    assert capfd.readouterr().err.strip() != ""  # first emit speaks

    tracker.emit_changed()  # nothing changed since
    assert capfd.readouterr().err.strip() == ""

    tracker.record("s1", p, "x = 2\n")
    tracker.emit_changed()  # a new read: speaks again
    assert '"total_reads":2' in capfd.readouterr().err


def test_emit_final_reports_every_nonempty_session(tmp_path: pathlib.Path, capfd) -> None:  # noqa: ANN001 -- pytest fixture
    p = tmp_path / "a.py"
    tracker = readstats.ReadStatsTracker()
    tracker.record("s1", p, "x = 1\n")
    tracker.record("s1", p, "x = 1\n")  # emit periodically, then shut down
    tracker.emit_changed()
    capfd.readouterr()

    # A clean-shutdown emit reports the final counts even if unchanged since the
    # last periodic emit, so the last window is never lost.
    tracker.emit_final()
    out = capfd.readouterr().err.strip()
    assert '"session":"s1"' in out
    assert '"total_reads":2' in out


def test_ranged_reads_of_one_file_are_each_novel(tmp_path: pathlib.Path) -> None:
    # The runtime hashes the payload the agent RECEIVED, not the whole file, so
    # two disjoint ranges of one file are two distinct (path, content) pairs. This
    # mirrors that by recording the two slices' content.
    p = tmp_path / "a.py"
    tracker = readstats.ReadStatsTracker()

    assert tracker.record("s1", p, "lines 1-100 body") is False
    assert tracker.record("s1", p, "lines 101-200 body") is False  # new page, not redundant
    assert tracker.snapshot("s1") == {"total_reads": 2, "redundant_reads": 0}
    # Re-reading the same range IS redundant.
    assert tracker.record("s1", p, "lines 1-100 body") is True


def test_record_digest_matches_record(tmp_path: pathlib.Path) -> None:
    # The async read path hashes off-loop with digest() then calls record_digest();
    # it must agree with the synchronous record() used by view.cat.
    p = tmp_path / "a.py"
    text = "x = 1\n"
    a = readstats.ReadStatsTracker()
    b = readstats.ReadStatsTracker()

    a.record("s1", p, text)
    b.record_digest("s1", readstats.digest(p, text))
    assert a.snapshot("s1") == b.snapshot("s1")
    # And a second read via each stays consistent (redundant both ways).
    assert a.record("s1", p, text) is True
    assert b.record_digest("s1", readstats.digest(p, text)) is True


def test_weird_session_id_stays_valid_json(tmp_path: pathlib.Path, capfd) -> None:  # noqa: ANN001 -- pytest fixture
    # A session id with a quote/backslash must not produce an unparseable line.
    p = tmp_path / "a.py"
    weird = 'ab"c\\d'
    tracker = readstats.ReadStatsTracker()
    tracker.record(weird, p, "x = 1\n")

    tracker.emit_changed()
    line = capfd.readouterr().err.strip()

    parsed = json.loads(line)  # must not raise
    assert parsed["session"] == weird
    assert parsed["event"] == "mcp_read_stats"
