"""Transcript signal extraction tests."""

import json
from pathlib import Path

from distiller import transcripts


def write_transcript(path: Path, records: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(json.dumps(r) for r in records))


def rec(rtype: str, content: object, ts: str = "2026-06-10T10:00:00Z", **extra: object) -> dict:
    return {
        "type": rtype,
        "message": {"role": rtype, "content": content},
        "timestamp": ts,
        "cwd": "/home/u/repo",
        "sessionId": "sess-1",
        **extra,
    }


def test_signals_and_outcome_success(tmp_path: Path) -> None:
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


def test_tool_error_failure_label(tmp_path: Path) -> None:
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


def test_meta_user_lines_and_sidechains_skipped(tmp_path: Path) -> None:
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


def test_scan_groups_by_repo_and_windows(tmp_path: Path) -> None:
    # cwd "/home/u/repo" -> repo slug "repo" (not the full path).
    new = tmp_path / "-home-u-repo" / "new.jsonl"
    write_transcript(new, [rec("user", "hello", ts="2026-06-10T10:00:00Z")])
    old = tmp_path / "-home-u-repo" / "old.jsonl"
    write_transcript(old, [rec("user", "ancient", ts="2020-01-01T00:00:00Z")])
    now = transcripts._parse_ts("2026-06-11T00:00:00Z")
    groups = transcripts.scan(tmp_path, days=7, now=now)
    assert list(groups) == ["repo"]
    assert [s.goal for s in groups["repo"]] == ["hello"]


def test_scan_collapses_clones_and_drops_scratch(tmp_path: Path) -> None:
    # Two clones of one repo at different paths collapse to one silo...
    a = tmp_path / "-home-u-Github-nox" / "a.jsonl"
    write_transcript(a, [rec("user", "in github clone", cwd="/home/u/Github/nox")])
    b = tmp_path / "-home-u-nox" / "b.jsonl"
    write_transcript(b, [rec("user", "in home clone", cwd="/home/u/nox")])
    # ...while a scratch cwd is dropped entirely.
    scratch = tmp_path / "-tmp-ix-distiller-x" / "c.jsonl"
    write_transcript(scratch, [rec("user", "scratch", cwd="/tmp/ix-distiller-x")])  # noqa: S108
    now = transcripts._parse_ts("2026-06-11T00:00:00Z")
    groups = transcripts.scan(tmp_path, days=3650, now=now)
    assert list(groups) == ["nox"]
    assert len(groups["nox"]) == 2


def test_repo_identity_heuristics() -> None:
    # clones collapse on basename
    assert transcripts.repo_identity("/home/u/Github/nox", "x") == "nox"
    assert transcripts.repo_identity("/home/u/nox", "x") == "nox"
    # worktree resolves to the repo, not the worktree name
    assert transcripts.repo_identity("/home/u/index/.claude/worktrees/feat-x", "x") == "index"
    # non-repo cwds are rejected
    assert transcripts.repo_identity("/tmp/ix-distiller-abc", "x") is None  # noqa: S108
    assert transcripts.repo_identity("/home/u", "x") is None
    assert transcripts.repo_identity("/home/u/Github", "x") is None
    assert transcripts.repo_identity("/home/u/.claude", "x") is None
    # dir-name fallback when cwd is absent
    assert transcripts.repo_identity(None, "-home-u-my-repo") == "repo"
    assert transcripts.project_slug("/home/u/my repo!") == "home-u-my-repo"


def test_is_meta_records_and_pasted_noise_never_become_the_goal(tmp_path: Path) -> None:
    # Records flagged isMeta (even plain-looking prose), pasted-TUI screens,
    # and paste placeholders all precede the typed request; none may win.
    path = tmp_path / "proj" / "s.jsonl"
    write_transcript(
        path,
        [
            rec("user", "Caveat: injected by the harness", isMeta=True),
            rec("user", "plain-looking but harness-flagged", isMeta=True),
            rec("user", "\u256d\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u256e\n\u2502 TUI \u2502\n\u2570\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u256f"),
            rec("user", "[Pasted text #1 +120 lines]"),
            rec("user", "real goal"),
        ],
    )
    session = transcripts.parse_session(path)
    assert session is not None
    assert session.goal == "real goal"


def test_resolve_cwd_prefers_the_recorded_cwd() -> None:
    assert transcripts.resolve_cwd("/home/u/repo", "-home-u-other") == "/home/u/repo"
    assert transcripts.resolve_cwd(None, "-home-u-repo") == "/home/u/repo"
