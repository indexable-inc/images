"""Transcript signal extraction tests."""

import json
from pathlib import Path

from distiller import transcripts


def write_transcript(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(json.dumps(r) for r in records))


def rec(rtype: str, content, ts: str = "2026-06-10T10:00:00Z", **extra) -> dict:
    return {
        "type": rtype,
        "message": {"role": rtype, "content": content},
        "timestamp": ts,
        "cwd": "/home/u/repo",
        "sessionId": "sess-1",
        **extra,
    }


def test_signals_and_outcome_success(tmp_path: Path):
    path = tmp_path / "proj" / "sess-1.jsonl"
    write_transcript(
        path,
        [
            {"type": "ai-title", "title": "x"},  # marker line, skipped
            rec("user", "fix the failing CI build"),
            rec("assistant", [{"type": "text", "text": "Looking into it."}]),
            rec("user", "no, don't touch the lockfile"),
            rec(
                "assistant",
                [{"type": "text", "text": "Done. Pushed to main, all tests pass."}],
                ts="2026-06-10T11:00:00Z",
            ),
            "not json at all",
        ],
    )
    session = transcripts.parse_session(path)
    assert session is not None
    assert session.session_id == "sess-1"
    assert session.goal == "fix the failing CI build"
    assert session.corrections == ["no, don't touch the lockfile"]
    assert session.outcome == "success"
    assert "Pushed to main" in (session.final_assistant or "")
    assert session.fingerprint().startswith("4:")


def test_tool_error_failure_label(tmp_path: Path):
    path = tmp_path / "proj" / "s.jsonl"
    write_transcript(
        path,
        [
            rec("user", "deploy it"),
            rec(
                "user",
                [{"type": "tool_result", "is_error": True, "content": "error: build failed"}],
                ts="2026-06-10T11:00:00Z",
            ),
        ],
    )
    session = transcripts.parse_session(path)
    assert session is not None
    assert session.errors == ["error: build failed"]
    assert session.outcome == "failure"


def test_meta_user_lines_and_sidechains_skipped(tmp_path: Path):
    path = tmp_path / "proj" / "s.jsonl"
    write_transcript(
        path,
        [
            rec("user", "<system-reminder>noise</system-reminder>"),
            rec("user", "real goal"),
            rec("user", "side goal", isSidechain=True),
        ],
    )
    session = transcripts.parse_session(path)
    assert session is not None
    assert session.goal == "real goal"


def test_scan_groups_by_cwd_and_windows(tmp_path: Path):
    new = tmp_path / "-home-u-repo" / "new.jsonl"
    write_transcript(new, [rec("user", "hello", ts="2026-06-10T10:00:00Z")])
    old = tmp_path / "-home-u-repo" / "old.jsonl"
    write_transcript(old, [rec("user", "ancient", ts="2020-01-01T00:00:00Z")])
    now = transcripts._parse_ts("2026-06-11T00:00:00Z")
    groups = transcripts.scan(tmp_path, days=7, now=now)
    assert list(groups) == ["/home/u/repo"]
    assert [s.goal for s in groups["/home/u/repo"]] == ["hello"]


def test_project_key_falls_back_to_dir_name():
    session = transcripts.Session(session_id="s", path="p")
    assert transcripts.project_key(session, "-home-u-my-repo") == "/home/u/my/repo"
    assert transcripts.project_slug("/home/u/my repo!") == "home-u-my-repo"
