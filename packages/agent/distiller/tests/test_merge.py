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
    kept = distill.retire_stale([fresh, stale], now=now, max_age_days=90.0)
    assert [i.id for i in kept] == ["df-fresh"]


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
