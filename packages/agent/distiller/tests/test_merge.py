"""Incremental-merge tests: stable ids, update-not-rewrite, caps."""

import itertools
import json
from collections.abc import Callable
from pathlib import Path

from distiller import distill
from distiller.types import Item, SessionRecord


def ids() -> Callable[[], str]:
    counter = itertools.count()
    return lambda: f"df-{next(counter):012x}"


def test_add_assigns_stable_id_and_clips_body() -> None:
    ops = [
        {
            "op": "add",
            "title": "Use gt sync",
            "body": " ".join(["word"] * 200),
            "outcome": "success",
            "scope": "shared",
            "sessions": ["s1"],
        }
    ]
    items = distill.apply_operations(
        [], ops, {"s1": SessionRecord(last_ts=100.0)}, now=1.0, id_factory=ids()
    )
    assert len(items) == 1
    item = items[0]
    assert item.id == "df-000000000000"
    assert len(item.body.split()) == 120
    assert item.sessions == ["s1"]
    assert item.evidence_from == item.evidence_to == 100.0


def test_update_keeps_id_and_merges_sessions() -> None:
    existing = [
        Item(
            id="df-aaa",
            title="Old title",
            body="Old body",
            outcome="mixed",
            scope="shared",
            sessions=["s1"],
            first_seen=1.0,
            last_updated=1.0,
        )
    ]
    ops = [{"op": "update", "id": "df-aaa", "body": "New body", "outcome": "success", "sessions": ["s2"]}]
    merged = distill.apply_operations(
        existing, ops, {"s2": SessionRecord(last_ts=50.0)}, now=2.0, id_factory=ids()
    )
    assert len(merged) == 1
    item = merged[0]
    assert item.id == "df-aaa"
    assert item.title == "Old title"  # untouched field survives
    assert item.body == "New body"
    assert item.outcome == "success"
    assert item.sessions == ["s1", "s2"]
    assert item.last_updated == 2.0
    # Input not mutated (anti-collapse: caller's previous state intact).
    assert existing[0].body == "Old body"


def test_unmentioned_items_survive_verbatim() -> None:
    existing = [
        Item(id="df-a", title="A", body="a", outcome="success", scope="shared", sessions=[]),
        Item(id="df-b", title="B", body="b", outcome="failure", scope="user", sessions=[]),
    ]
    ops = [{"op": "update", "id": "df-a", "body": "a2"}]
    merged = distill.apply_operations(existing, ops, {}, now=3.0, id_factory=ids())
    by_id = {i.id: i for i in merged}
    assert by_id["df-b"].body == "b"  # never regenerated
    assert by_id["df-a"].body == "a2"


def test_add_cap_and_duplicate_title_guard() -> None:
    ops = [
        {"op": "add", "title": f"T{i}", "body": "b", "outcome": "success", "scope": "shared"}
        for i in range(5)
    ] + [{"op": "add", "title": "t0", "body": "dup", "outcome": "success", "scope": "shared"}]
    merged = distill.apply_operations([], ops, {}, now=1.0, id_factory=ids(), max_new=3)
    assert len(merged) == 3  # cap enforced, case-insensitive dup dropped


def test_unknown_update_and_garbage_ops_ignored() -> None:
    merged = distill.apply_operations(
        [], [{"op": "update", "id": "nope"}, "garbage", {"op": "drop"}], {}, now=1.0, id_factory=ids()
    )
    assert merged == []


def test_delete_op_removes_item_with_cap() -> None:
    existing = [
        Item(id=f"df-{c}", title=f"T{c}", body="b", outcome="success", scope="shared", sessions=[])
        for c in "abcd"
    ]
    ops = [
        {"op": "delete", "id": "df-a"},
        {"op": "delete", "id": "df-b"},
        {"op": "delete", "id": "df-c"},
        {"op": "delete", "id": "df-d"},  # beyond cap -> ignored
        {"op": "delete", "id": "nope"},  # unknown id -> no-op
    ]
    merged = distill.apply_operations(existing, ops, {}, now=1.0, id_factory=ids(), max_delete=3)
    assert {i.id for i in merged} == {"df-d"}  # exactly 3 deleted, cap held, unknown ignored


def test_retire_stale_drops_old_unreevidenced_items() -> None:
    now = 1_000_000_000.0
    day = 86400.0
    fresh = Item(
        id="df-fresh", title="F", body="b", outcome="success", scope="shared",
        sessions=[], last_updated=now - 10 * day, evidence_to=now - 10 * day,
    )
    stale = Item(
        id="df-stale", title="S", body="b", outcome="success", scope="shared",
        sessions=[], last_updated=now - 200 * day, evidence_to=now - 200 * day,
    )
    # Refreshed this run by an update op that carried no in-window session: old
    # evidence_to but fresh last_updated. Must survive (freshest signal wins).
    refreshed = Item(
        id="df-refreshed", title="R", body="b", outcome="success", scope="shared",
        sessions=[], last_updated=now, evidence_to=now - 200 * day,
    )
    kept = distill.retire_stale([fresh, stale, refreshed], now=now, max_age_days=90.0)
    assert [i.id for i in kept] == ["df-fresh", "df-refreshed"]


def test_retire_stale_keeps_items_without_provenance_stamps() -> None:
    """A legacy/migrated item with no evidence_to or last_updated has unknown age
    and must be kept, not retired on the first run (migration must not wipe it).
    """
    now = 1_000_000_000.0
    # The pre-provenance shape: last_updated defaults to 0.0 and evidence_to is
    # absent, so recency() has no usable signal.
    no_stamp = Item(
        id="df-legacy", title="L", body="b", outcome="mixed", scope="shared",
        sessions=[], evidence_to=None,
    )
    assert no_stamp.last_updated == 0.0
    kept = distill.retire_stale([no_stamp], now=now, max_age_days=90.0)
    assert [i.id for i in kept] == ["df-legacy"]


def test_extract_json_tolerates_fences() -> None:
    text = "Here you go:\n```json\n{\"operations\": []}\n```\nDone."
    assert distill._extract_json(text) == {"operations": []}


def test_prompt_starts_with_sentinel() -> None:
    # cli.run drops sessions whose first user message starts with the
    # sentinel (self-distillation guard); the prompt must keep that coupling.
    prompt = distill.build_prompt("/p", [], ["### session x"])
    assert prompt.startswith(distill.PROMPT_SENTINEL)


# ---------------------------------------------------------------------------
# Regression: state.load() must not crash on legacy/invalid state files.
# ---------------------------------------------------------------------------


def test_load_legacy_item_missing_scope_and_outcome(tmp_path: Path) -> None:
    """A state file whose items[] omit 'scope'/'outcome' (total=False legacy)
    must load cleanly with the old-loader defaults, not raise ValidationError.
    """
    from distiller import state as state_mod

    legacy = {
        "project": "/home/u/repo",
        "items": [
            # Missing 'scope' and 'outcome' -- what the old TypedDict produced.
            {"id": "df-aaa", "title": "Old lesson", "body": "Do the thing."}
        ],
        "distilled_sessions": {},
        "session_outcomes": {},
    }
    state_path = tmp_path / "state" / "u" / "repo.json"
    state_path.parent.mkdir(parents=True)
    state_path.write_text(json.dumps(legacy))

    loaded = state_mod.load(tmp_path, "u", "repo")
    assert len(loaded.items) == 1
    item = loaded.items[0]
    assert item.id == "df-aaa"
    # Defaults must match the old .get() fallbacks from corpus.py.
    assert item.scope == "shared"
    assert item.outcome == "mixed"


def test_load_schema_invalid_json_returns_empty(tmp_path: Path) -> None:
    """A state file that is valid JSON but fails pydantic schema validation
    (e.g. items is a string instead of a list) returns an empty State rather
    than propagating a ValidationError.
    """
    from distiller import state as state_mod

    bad = {"project": "/p", "items": "not-a-list"}
    state_path = tmp_path / "state" / "u" / "repo.json"
    state_path.parent.mkdir(parents=True)
    state_path.write_text(json.dumps(bad))

    loaded = state_mod.load(tmp_path, "u", "repo")
    assert loaded.items == []
    assert loaded.project is None


def test_legacy_state_slugs_dedupes_newest_first() -> None:
    """The legacy per-cwd slugs are the old state filenames, newest cwd first."""
    from distiller.transcripts import Session, legacy_state_slugs

    sessions = [
        Session(session_id="a", path="a.jsonl", cwd="/home/u/repo", last_ts=100.0),
        Session(session_id="b", path="b.jsonl", cwd="/home/u/Github/repo", last_ts=200.0),
        Session(session_id="c", path="c.jsonl", cwd="/home/u/repo", last_ts=50.0),
    ]
    # Newest distinct cwd first; the duplicate /home/u/repo collapses.
    assert legacy_state_slugs(sessions) == ["home-u-Github-repo", "home-u-repo"]


def test_legacy_state_slugs_falls_back_to_transcript_dir() -> None:
    """A cwd-less session keyed off the decoded transcript dir, like old scan()."""
    from distiller.transcripts import Session, legacy_state_slugs

    # No recorded cwd: old _resolve_path used the decoded `-home-u-repo` dir name.
    sessions = [
        Session(
            session_id="a",
            path="/x/.claude/projects/-home-u-repo/a.jsonl",
            cwd=None,
            last_ts=10.0,
        ),
    ]
    assert legacy_state_slugs(sessions) == ["home-u-repo"]


def test_load_migrates_legacy_cwd_state(tmp_path: Path) -> None:
    """First run after repo-keying adopts the old per-cwd state file so that
    previously learned items are not silently dropped from the rewritten corpus.
    """
    from distiller import state as state_mod

    legacy = {
        "project": "/home/u/repo",
        "items": [{"id": "df-old", "title": "Kept lesson", "body": "Do the thing."}],
        "distilled_sessions": {"s1": "3:100"},
        "session_outcomes": {},
    }
    # Old key was the raw cwd slug; new canonical key is the repo basename.
    legacy_path = tmp_path / "state" / "u" / "home-u-repo.json"
    legacy_path.parent.mkdir(parents=True)
    legacy_path.write_text(json.dumps(legacy))
    assert not (tmp_path / "state" / "u" / "repo.json").is_file()

    loaded = state_mod.load(tmp_path, "u", "repo", legacy_slugs=["home-u-repo"])
    assert [item.id for item in loaded.items] == ["df-old"]
    assert loaded.distilled_sessions == {"s1": "3:100"}


def test_load_prefers_canonical_over_legacy(tmp_path: Path) -> None:
    """Once the canonical file exists, the legacy file is ignored (no re-merge)."""
    from distiller import state as state_mod

    base = tmp_path / "state" / "u"
    base.mkdir(parents=True)
    (base / "repo.json").write_text(
        json.dumps(
            {
                "project": "repo",
                "items": [{"id": "df-new", "title": "Current", "body": "x"}],
                "distilled_sessions": {},
                "session_outcomes": {},
            }
        )
    )
    (base / "home-u-repo.json").write_text(
        json.dumps(
            {
                "project": "/home/u/repo",
                "items": [{"id": "df-old", "title": "Stale", "body": "y"}],
                "distilled_sessions": {},
                "session_outcomes": {},
            }
        )
    )

    loaded = state_mod.load(tmp_path, "u", "repo", legacy_slugs=["home-u-repo"])
    assert [item.id for item in loaded.items] == ["df-new"]
