"""``claude_history.search`` over a fixture transcript tree (issue #2245).

One ranked row per matching session: id, un-munged cwd, start/end timestamps,
hit count, and the first *real* user message. Harness-injected meta records
(``isMeta`` / ``<...>`` / ``Caveat:``), tool-result-only entries, and
pasted-TUI noise still count as grep hits, but must never surface as the
opening message. Runs against real ripgrep (the fsearch backend) and skips
cleanly when ``rg`` is not on PATH.
"""

from __future__ import annotations

import asyncio
import json
import shutil
from datetime import UTC, datetime
from pathlib import Path

import pytest

import claude_history

pytestmark = pytest.mark.skipif(shutil.which("rg") is None, reason="rg not on PATH")

NEEDLE = "golden snapshot"


def _line(
    rtype: str,
    content: object,
    ts: str,
    session: str,
    cwd: str = "/home/u/proj",
    **extra: object,
) -> str:
    return json.dumps(
        {
            "type": rtype,
            "uuid": f"{session}:{ts}",
            "sessionId": session,
            "timestamp": ts,
            "cwd": cwd,
            "gitBranch": "main",
            "message": {"role": rtype, "content": content},
            **extra,
        }
    )


def _fixture_root(tmp_path: Path) -> Path:
    """Two sessions matching NEEDLE (5 hits vs 1) plus a subagent transcript."""
    root = tmp_path / "projects"
    proj = root / "-home-u-proj"
    proj.mkdir(parents=True)
    tui_paste = (
        "\u256d\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u256e\n"
        f"\u2502 {NEEDLE} \u2502\n"
        "\u2570\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u256f"
    )
    (proj / "sess-a.jsonl").write_text(
        "\n".join(
            [
                # A hit inside harness-injected meta: counted, never the goal.
                _line("user", f"Caveat: harness noise about the {NEEDLE}", "2026-06-10T10:00:00Z", "sess-a", isMeta=True),
                # Tool-result-only user line: not a real user message.
                _line(
                    "user",
                    [{"type": "tool_result", "tool_use_id": "t1", "content": f"{NEEDLE} in tool output"}],
                    "2026-06-10T10:00:30Z",
                    "sess-a",
                ),
                # Pasted-TUI noise carrying the needle: counted, never the goal.
                _line("user", tui_paste, "2026-06-10T10:01:00Z", "sess-a"),
                _line("user", f"please fix the {NEEDLE} test", "2026-06-10T10:02:00Z", "sess-a"),
                _line(
                    "assistant",
                    [{"type": "text", "text": f"Looking at the {NEEDLE} fixture now."}],
                    "2026-06-10T11:00:00Z",
                    "sess-a",
                ),
            ]
        )
    )
    other = root / "-home-u-other"
    other.mkdir()
    (other / "sess-b.jsonl").write_text(
        "\n".join(
            [
                _line("user", "unrelated question", "2026-06-11T09:00:00Z", "sess-b", cwd="/home/u/other"),
                _line(
                    "assistant",
                    [{"type": "text", "text": f"that reminds me of the {NEEDLE}"}],
                    "2026-06-11T09:05:00Z",
                    "sess-b",
                    cwd="/home/u/other",
                ),
            ]
        )
    )
    sub = proj / "sess-a" / "subagents"
    sub.mkdir(parents=True)
    (sub / "agent-1.jsonl").write_text(
        _line("user", f"subagent chatter about the {NEEDLE}", "2026-06-10T10:30:00Z", "agent-1")
    )
    return root


def test_ranked_sessions_with_first_real_user_message(tmp_path: Path) -> None:
    frame = asyncio.run(claude_history.search(NEEDLE, _fixture_root(tmp_path)))
    assert frame["session_id"].to_list() == ["sess-a", "sess-b"], frame
    assert frame["hits"].to_list() == [5, 1]

    top = frame.row(0, named=True)
    # Meta, tool-result-only, and pasted-TUI lines all matched, yet the first
    # REAL user message is the typed request.
    assert top["first_user_message"] == f"please fix the {NEEDLE} test"
    assert top["cwd"] == "/home/u/proj"  # un-munged, from the records
    assert top["started_at"] == datetime(2026, 6, 10, 10, 0, tzinfo=UTC)
    assert top["ended_at"] == datetime(2026, 6, 10, 11, 0, tzinfo=UTC)
    assert top["git_branch"] == "main"
    assert top["path"].endswith("sess-a.jsonl")
    assert frame.row(1, named=True)["cwd"] == "/home/u/other"


def test_subagent_transcripts_are_folded_out_by_default(tmp_path: Path) -> None:
    root = _fixture_root(tmp_path)
    default = asyncio.run(claude_history.search(NEEDLE, root))
    assert "agent-1" not in default["session_id"].to_list()

    included = asyncio.run(claude_history.search(NEEDLE, root, include_subagents=True))
    assert "agent-1" in included["session_id"].to_list()


def test_no_match_returns_an_empty_typed_frame(tmp_path: Path) -> None:
    frame = asyncio.run(claude_history.search("no-such-needle-xyz", _fixture_root(tmp_path)))
    assert frame.is_empty()
    assert frame.columns == list(claude_history._SCHEMA)
    assert not getattr(frame, "truncated", False)


def test_limit_caps_returned_sessions(tmp_path: Path) -> None:
    frame = asyncio.run(claude_history.search(NEEDLE, _fixture_root(tmp_path), limit=1))
    assert frame["session_id"].to_list() == ["sess-a"]
