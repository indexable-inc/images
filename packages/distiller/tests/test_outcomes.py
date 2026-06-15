"""Outcome-labeling tests: verdict normalization, sessions slice, e2e cli run."""

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

import polars as pl
import pytest

from distiller import cli, corpus, distill, transcripts


def session(sid: str, **overrides) -> transcripts.Session:
    base = dict(session_id=sid, path=f"/t/{sid}.jsonl", message_count=20, outcome="mixed")
    base.update(overrides)
    return transcripts.Session(**base)


def test_session_verdicts_normalize_and_fallback():
    sessions = [
        session("s-ok", outcome="success"),
        session("s-bad", outcome="failure"),
        session("s-quiet", outcome="unknown", message_count=3),
        session("s-long", outcome="unknown", message_count=40),
    ]
    outcomes = [
        {"session_id": "s-ok", "label": "success", "reason": "goal shipped"},
        {"session_id": "s-bad", "label": "exploded"},  # off-label -> fallback
        {"session_id": "s-invented", "label": "failure", "reason": "n/a"},  # unknown id
        "garbage",
    ]
    verdicts = distill.session_verdicts(outcomes, sessions)
    assert set(verdicts) == {"s-ok", "s-bad", "s-quiet", "s-long"}
    assert verdicts["s-ok"] == {"label": "success", "reason": "goal shipped"}
    assert verdicts["s-bad"]["label"] == "failure"  # heuristic fallback
    assert "fallback" in verdicts["s-bad"]["reason"]
    assert verdicts["s-quiet"]["label"] == "abandoned"  # tiny unknown session
    assert verdicts["s-long"]["label"] == "partial"
    assert all(v["label"] in distill.SESSION_LABELS for v in verdicts.values())


def test_envelope_result_handles_object_and_event_array():
    # Older CLIs: one {"result": ...} object.
    assert distill._envelope_result({"result": "{}"}) == "{}"
    # claude >= 2.1: full event array, final entry type=result.
    events = [
        {"type": "system", "subtype": "init"},
        {"type": "assistant", "message": {"content": []}},
        {"type": "result", "subtype": "success", "result": '{"operations": []}'},
    ]
    assert distill._envelope_result(events) == '{"operations": []}'
    assert distill._envelope_result([{"type": "assistant"}]) == ""
    assert distill._envelope_result(42) == ""


def test_prompt_keeps_sentinel_and_requests_verdicts():
    prompt = distill.build_prompt("/p", [], ["### session x"])
    assert prompt.startswith(distill.PROMPT_SENTINEL)
    assert "session_outcomes" in prompt
    for label in distill.SESSION_LABELS:
        assert f'"{label}"' in prompt


def make_rec(**overrides) -> dict:
    rec = {
        "label": "failure",
        "reason": "build never recovered after the lockfile edit",
        "goal": "fix the failing CI build",
        "turns": 42,
        "duration_s": 1080,
        "models": ["claude-haiku-4-5-20251001", "claude-sonnet-4-5"],
        "errors": 3,
        "corrections": 1,
        "last_ts": 1_700_000_000.0,
    }
    rec.update(overrides)
    return rec


def test_session_row_contract():
    row = corpus.session_row("sess-1", make_rec(), "/home/u/repo", "hostx", "useru")
    assert row["source"] == corpus.SESSIONS_SOURCE == "session_outcomes"
    assert row["external_id"] == "session_outcomes:useru:home-u-repo:sess-1"
    assert row["content_hash"] == corpus.hash_body(row["body"].encode())
    assert row["timestamp"] == 1_700_000_000
    assert row["title"].startswith("[failure] fix the failing CI build")
    assert "build never recovered" in row["body"]
    assert "turns: 42" in row["body"]
    meta = json.loads(row["meta_json"])
    assert meta["label"] == "failure"
    assert meta["turns"] == 42
    assert meta["duration_s"] == 1080
    assert meta["models"] == "claude-haiku-4-5-20251001,claude-sonnet-4-5"
    assert meta["session_id"] == "sess-1"
    assert meta["user"] == "useru" and meta["host"] == "hostx"


def test_session_slice_roundtrip(tmp_path: Path):
    rows = [
        corpus.session_row(f"s-{i}", make_rec(label=label), "/p", "h", "u")
        for i, label in enumerate(("success", "partial", "failure", "abandoned"))
    ]
    corpus.write_slice(rows, tmp_path / "slice")
    assert corpus.validate_slice(tmp_path / "slice", source=corpus.SESSIONS_SOURCE) == 4
    # The default (distilled_facts) source check must reject this slice.
    with pytest.raises(corpus.ContractError, match="source"):
        corpus.validate_slice(tmp_path / "slice")


def test_item_row_marks_failure_derived():
    item = {
        "id": "df-1",
        "title": "Never edit the lockfile by hand",
        "body": "Run `cargo update -p <crate>` instead.",
        "outcome": "failure",
        "scope": "shared",
        "sessions": ["s-bad", "s-ok"],
        "last_updated": 1_700_000_100.0,
    }
    labels = {"s-bad": "failure", "s-ok": "success"}
    row = corpus.item_row(item, "/p", "h", "u", session_labels=labels)
    meta = json.loads(row["meta_json"])
    assert meta["session_labels"] == "failure,success"
    assert meta["failure_derived"] is True
    assert "session-labels: failure, success" in row["body"]
    # Without label info the meta stays as before (no empty keys).
    bare = json.loads(corpus.item_row(item, "/p", "h", "u")["meta_json"])
    assert "session_labels" not in bare and "failure_derived" not in bare


def _ts(minutes: int) -> str:
    stamp = datetime.now(timezone.utc) - timedelta(hours=2) + timedelta(minutes=minutes)
    return stamp.strftime("%Y-%m-%dT%H:%M:%SZ")


def _transcript(path: Path, sid: str, goal: str, records: list[dict]) -> None:
    base = {"cwd": "/home/u/repo", "sessionId": sid}
    lines = [
        {**base, "type": "user", "message": {"role": "user", "content": goal}, "timestamp": _ts(0)}
    ] + [{**base, **rec} for rec in records]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(json.dumps(line) for line in lines))


def _fake_claude(tmp_path: Path, reply: dict) -> Path:
    """Mock `claude -p`: swallow stdin, print a canned JSON envelope."""
    reply_path = tmp_path / "reply.json"
    reply_path.write_text(json.dumps({"result": json.dumps(reply)}))
    script = tmp_path / "claude"
    script.write_text(f"#!/bin/sh\ncat >/dev/null\ncat {reply_path}\n")
    script.chmod(0o755)
    return script


def test_cli_end_to_end_writes_both_slices(tmp_path: Path, capsys):
    root = tmp_path / "projects" / "-home-u-repo"
    _transcript(
        root / "sess-ok.jsonl",
        "sess-ok",
        "add a retry to the uploader",
        [
            {
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "model": "claude-test-1",
                    "content": [{"type": "text", "text": "Done. Pushed to main."}],
                },
                "timestamp": _ts(30),
            }
        ],
    )
    _transcript(
        root / "sess-bad.jsonl",
        "sess-bad",
        "fix the failing CI build",
        [
            {
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [
                        {"type": "tool_result", "is_error": True, "content": "error: boom"}
                    ],
                },
                "timestamp": _ts(10),
            }
        ],
    )
    # The distiller's own headless call: must be sentinel-filtered out.
    _transcript(
        root / "sess-self.jsonl",
        "sess-self",
        distill.PROMPT_SENTINEL + " of strategy-level lessons ...",
        [],
    )
    reply = {
        "operations": [
            {
                "op": "add",
                "title": "Never edit the lockfile by hand",
                "body": "Run `cargo update -p <crate>`; hand edits broke CI in sess-bad.",
                "outcome": "failure",
                "scope": "shared",
                "sessions": ["sess-bad"],
            }
        ],
        "session_outcomes": [
            {"session_id": "sess-ok", "label": "success", "reason": "pushed to main"},
            {"session_id": "sess-bad", "label": "failure", "reason": "CI never recovered"},
            {"session_id": "sess-self", "label": "success", "reason": "must not appear"},
        ],
    }
    args = cli.build_parser().parse_args(
        [
            "--days", "7",
            "--user", "u",
            "--host", "h",
            "--claude-root", str(tmp_path / "projects"),
            "--out", str(tmp_path / "out"),
            "--claude-bin", str(_fake_claude(tmp_path, reply)),
        ]
    )
    assert cli.run(args) == 0
    out = capsys.readouterr().out
    assert "outcome label distribution: failure=1, success=1" in out

    base = tmp_path / "out" / "corpus" / "host=h" / "user=u"
    facts_dir = base / "source=distilled_facts"
    sessions_dir = base / "source=session_outcomes"
    assert corpus.validate_slice(facts_dir) == 1
    assert corpus.validate_slice(sessions_dir, source=corpus.SESSIONS_SOURCE) == 2

    sessions_frame = pl.read_parquet(sessions_dir / "data.parquet")
    metas = {m["session_id"]: m for m in map(json.loads, sessions_frame["meta_json"])}
    assert set(metas) == {"sess-ok", "sess-bad"}  # sentinel session filtered
    assert metas["sess-ok"]["label"] == "success"
    assert metas["sess-ok"]["models"] == "claude-test-1"
    assert metas["sess-ok"]["turns"] == 2
    assert metas["sess-ok"]["duration_s"] == 30 * 60
    assert metas["sess-bad"]["label"] == "failure"
    assert metas["sess-bad"]["reason"] == "CI never recovered"

    facts_frame = pl.read_parquet(facts_dir / "data.parquet")
    lesson_meta = json.loads(facts_frame["meta_json"][0])
    assert lesson_meta["session_labels"] == "failure"
    assert lesson_meta["failure_derived"] is True

    # Second run with nothing new: verdicts survive via state, slice intact.
    assert cli.run(args) == 0
    assert corpus.validate_slice(sessions_dir, source=corpus.SESSIONS_SOURCE) == 2
