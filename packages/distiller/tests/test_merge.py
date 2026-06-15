"""Incremental-merge tests: stable ids, update-not-rewrite, caps."""

import itertools

from distiller import distill


def ids():
    counter = itertools.count()
    return lambda: f"df-{next(counter):012x}"


def test_add_assigns_stable_id_and_clips_body():
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
    items = distill.apply_operations([], ops, {"s1": {"last_ts": 100.0}}, now=1.0, id_factory=ids())
    assert len(items) == 1
    item = items[0]
    assert item["id"] == "df-000000000000"
    assert len(item["body"].split()) == 120
    assert item["sessions"] == ["s1"]
    assert item["evidence_from"] == item["evidence_to"] == 100.0


def test_update_keeps_id_and_merges_sessions():
    existing = [
        {
            "id": "df-aaa",
            "title": "Old title",
            "body": "Old body",
            "outcome": "mixed",
            "scope": "shared",
            "sessions": ["s1"],
            "first_seen": 1.0,
            "last_updated": 1.0,
        }
    ]
    ops = [{"op": "update", "id": "df-aaa", "body": "New body", "outcome": "success", "sessions": ["s2"]}]
    merged = distill.apply_operations(
        existing, ops, {"s2": {"last_ts": 50.0}}, now=2.0, id_factory=ids()
    )
    assert len(merged) == 1
    item = merged[0]
    assert item["id"] == "df-aaa"
    assert item["title"] == "Old title"  # untouched field survives
    assert item["body"] == "New body"
    assert item["outcome"] == "success"
    assert item["sessions"] == ["s1", "s2"]
    assert item["last_updated"] == 2.0
    # Input not mutated (anti-collapse: caller's previous state intact).
    assert existing[0]["body"] == "Old body"


def test_unmentioned_items_survive_verbatim():
    existing = [
        {"id": "df-a", "title": "A", "body": "a", "outcome": "success", "scope": "shared", "sessions": []},
        {"id": "df-b", "title": "B", "body": "b", "outcome": "failure", "scope": "user", "sessions": []},
    ]
    ops = [{"op": "update", "id": "df-a", "body": "a2"}]
    merged = distill.apply_operations(existing, ops, {}, now=3.0, id_factory=ids())
    by_id = {i["id"]: i for i in merged}
    assert by_id["df-b"]["body"] == "b"  # never regenerated
    assert by_id["df-a"]["body"] == "a2"


def test_add_cap_and_duplicate_title_guard():
    ops = [
        {"op": "add", "title": f"T{i}", "body": "b", "outcome": "success", "scope": "shared"}
        for i in range(5)
    ] + [{"op": "add", "title": "t0", "body": "dup", "outcome": "success", "scope": "shared"}]
    merged = distill.apply_operations([], ops, {}, now=1.0, id_factory=ids(), max_new=3)
    assert len(merged) == 3  # cap enforced, case-insensitive dup dropped


def test_unknown_update_and_garbage_ops_ignored():
    merged = distill.apply_operations(
        [], [{"op": "update", "id": "nope"}, "garbage", {"op": "drop"}], {}, now=1.0, id_factory=ids()
    )
    assert merged == []


def test_extract_json_tolerates_fences():
    text = "Here you go:\n```json\n{\"operations\": []}\n```\nDone."
    assert distill._extract_json(text) == {"operations": []}


def test_prompt_starts_with_sentinel():
    # cli.run drops sessions whose first user message starts with the
    # sentinel (self-distillation guard); the prompt must keep that coupling.
    prompt = distill.build_prompt("/p", [], ["### session x"])
    assert prompt.startswith(distill.PROMPT_SENTINEL)
