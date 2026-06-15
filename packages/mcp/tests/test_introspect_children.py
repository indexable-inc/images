"""Recursive namespace children (the dashboard's drill-in for containers).

`namespace_rows` now hangs a `children` list off any expandable container
(mapping, sequence, plain object) so the Namespace view can expand it in place.
These tests pin the contract the UI relies on: the recursive row shape, the
`max_depth` and `breadth` bounds, the elision marker, which kinds stay leaves,
and — most important for a tool that runs inside the live kernel — that a value
whose introspection misbehaves degrades to a plain row rather than raising.

Imports only :mod:`ix_notebook_mcp.introspect`, which is pure stdlib, so this
file runs without the MCP/kernel dependency stack the heavier suites need.
"""

from __future__ import annotations

import os

from ix_notebook_mcp import introspect


def _only_row(value, **kwargs) -> dict:
    """The single row for a one-name namespace (the common test fixture)."""
    rows = introspect.namespace_rows({"v": value}, **kwargs)
    assert len(rows) == 1
    return rows[0]


# --------------------------------------------------------------------------- #
# recursion + the max_depth boundary
# --------------------------------------------------------------------------- #


def test_nested_dict_yields_children_with_a_depth_boundary() -> None:
    # {"a": 1, "b": {"c": 2}}: the outer dict (depth 0) and the inner dict
    # (depth 1) both expand, but the inner dict's child sits at depth 2 == max,
    # so the inner dict itself carries children while a third level would not.
    row = _only_row({"a": 1, "b": {"c": 2}}, max_depth=2)
    assert row["kind"] == "mapping"
    assert "children" in row

    by_name = {child["name"]: child for child in row["children"]}
    assert by_name["a"]["repr"] == "1"
    inner = by_name["b"]
    assert inner["kind"] == "mapping"
    # The inner dict is at depth 1, so it still gets its own children (depth 2).
    assert "children" in inner
    assert inner["children"][0]["name"] == "c"
    # ...but those depth-2 children are leaves: nothing emitted at depth 3.
    assert "children" not in inner["children"][0]


def test_max_depth_one_stops_after_the_top_level() -> None:
    # With max_depth=1 the top dict expands (its children are depth 1 == max),
    # but the nested dict child is a leaf — no depth-2 descent.
    row = _only_row({"a": 1, "b": {"c": 2}}, max_depth=1)
    inner = {child["name"]: child for child in row["children"]}["b"]
    assert inner["kind"] == "mapping"
    assert "children" not in inner


# --------------------------------------------------------------------------- #
# sequences: indexed names for lists, unordered names for sets
# --------------------------------------------------------------------------- #


def test_list_yields_indexed_children() -> None:
    row = _only_row([10, 20, 30])
    assert row["kind"] == "sequence"
    names = [child["name"] for child in row["children"]]
    assert names == ["[0]", "[1]", "[2]"]
    assert [child["repr"] for child in row["children"]] == ["10", "20", "30"]


def test_tuple_is_also_indexed() -> None:
    row = _only_row((1, 2))
    assert [child["name"] for child in row["children"]] == ["[0]", "[1]"]


def test_set_children_are_named_by_repr_not_index() -> None:
    row = _only_row({42})
    assert row["kind"] == "sequence"
    # Sets are unordered, so the child name is the bounded element repr.
    assert row["children"][0]["name"] == "42"


# --------------------------------------------------------------------------- #
# the breadth cap + elision marker
# --------------------------------------------------------------------------- #


def test_list_over_breadth_emits_cap_plus_one_elision_marker() -> None:
    cap = 5
    row = _only_row(list(range(100)), breadth=cap)
    children = row["children"]
    # Exactly cap real children, then a single elision marker.
    assert len(children) == cap + 1
    marker = children[-1]
    assert marker == {
        "name": "…",
        "type": "",
        "kind": "object",
        "repr": "+95 more",
        "size": 0,
        "shape": "",
    }
    assert "children" not in marker


def test_dict_over_breadth_emits_cap_plus_one_elision_marker() -> None:
    cap = 4
    big = {f"k{i}": i for i in range(20)}
    row = _only_row(big, breadth=cap)
    children = row["children"]
    assert len(children) == cap + 1
    assert children[-1]["name"] == "…"
    assert children[-1]["repr"] == "+16 more"


def test_container_exactly_at_breadth_has_no_marker() -> None:
    cap = 3
    row = _only_row([1, 2, 3], breadth=cap)
    # Exactly cap entries fit, so there is nothing to elide.
    assert len(row["children"]) == cap
    assert all(child["name"] != "…" for child in row["children"])


# --------------------------------------------------------------------------- #
# objects: public instance attrs only, no methods/dunders
# --------------------------------------------------------------------------- #


class _Thing:
    def __init__(self) -> None:
        self.x = 1
        self.data = {"k": 2}

    def method(self) -> None:  # behavior, not data — must not appear
        pass


def test_object_children_are_public_instance_attributes() -> None:
    row = _only_row(_Thing())
    assert row["kind"] == "object"
    names = {child["name"] for child in row["children"]}
    assert names == {"x", "data"}
    # Nested container attrs recurse like any other value.
    data = {child["name"]: child for child in row["children"]}["data"]
    assert data["kind"] == "mapping"
    assert data["children"][0]["name"] == "k"


def test_object_without_vars_has_no_children() -> None:
    class _Slotted:
        __slots__ = ()

    row = _only_row(_Slotted())
    assert "children" not in row


def test_object_dunder_attrs_do_not_consume_breadth_or_hide_public_attrs() -> None:
    # Regression: dunder instance attrs sitting in __dict__ must be filtered
    # *before* the breadth window is applied. A naive islice(__dict__, breadth+1)
    # lets dunders eat slots, silently dropping real public attrs (and skipping the
    # elision marker), and inflates the `+N more` count with the hidden members.
    class _C:
        pass

    obj = _C()
    # Two leading dunder instance attrs, then four public ones (insertion order is
    # preserved, so the dunders fall inside a naive window).
    obj.__dict__["__hidden1"] = 1
    obj.__dict__["__hidden2"] = 2
    obj.a = 10
    obj.b = 20
    obj.c = 30
    obj.d = 40

    row = _only_row(obj, breadth=2)
    children = row["children"]
    # Exactly breadth real public attrs survive, then one elision marker — the
    # dunders neither appear nor consume a slot.
    assert [c["name"] for c in children[:2]] == ["a", "b"]
    marker = children[-1]
    assert marker["name"] == "…"
    # The count reflects only the unrendered *eligible* attrs (c, d) — not the
    # hidden dunders.
    assert marker["repr"] == "+2 more"
    assert len(children) == 3


# --------------------------------------------------------------------------- #
# leaf kinds: never carry children
# --------------------------------------------------------------------------- #


def test_non_expandable_kinds_have_no_children() -> None:
    def fn() -> None:
        pass

    for value in (5, "hello", os, fn, _Thing):  # int, text, module, function, class
        row = _only_row(value)
        assert "children" not in row, (row["kind"], row["name"])


# --------------------------------------------------------------------------- #
# defensiveness: a misbehaving value/attr must never escape namespace_rows
# --------------------------------------------------------------------------- #


class _Exploding:
    """An object whose property raises and whose data attr is a dict — the row
    must still come back, the property must simply not show up."""

    def __init__(self) -> None:
        self.safe = 1

    @property
    def boom(self):  # a property that raises if anyone touches it
        raise RuntimeError("do not read me")


def test_property_that_raises_does_not_blow_up_namespace_rows() -> None:
    # Should not raise. The exploding property lives on the class, not in vars(),
    # so it is never read; the safe instance attr still surfaces.
    row = _only_row(_Exploding())
    assert row["kind"] == "object"
    names = {child["name"] for child in row["children"]}
    assert "safe" in names
    assert "boom" not in names


def test_value_whose_describe_raises_degrades_to_no_row() -> None:
    class _Hostile:
        def __getattribute__(self, name):  # type: ignore[override]
            raise RuntimeError("everything explodes")

    # The top-level value's introspection raises, so it is dropped, not fatal.
    rows = introspect.namespace_rows({"ok": 1, "bad": _Hostile()})
    names = {row["name"] for row in rows}
    assert "ok" in names
    assert "bad" not in names


# --------------------------------------------------------------------------- #
# deep nesting + the global row budget
# --------------------------------------------------------------------------- #


def _count_rows(rows: list[dict]) -> int:
    """Total rows in a tree, including elision markers."""
    return sum(1 + _count_rows(r.get("children") or []) for r in rows)


def test_deep_nesting_expands_past_the_old_depth_two_limit() -> None:
    # {"a": {"b": {"c": {"d": 1}}}} with the default depth: 'c' lives at depth 3
    # and still expands to 'd' at depth 4 — impossible under the old _MAX_DEPTH=2.
    row = _only_row({"a": {"b": {"c": {"d": 1}}}})
    cur = row
    for key in ("a", "b", "c"):
        cur = {child["name"]: child for child in cur["children"]}[key]
    assert "children" in cur
    assert cur["children"][0]["name"] == "d"


def test_global_budget_bounds_total_rows() -> None:
    # A structure that, unbounded, is exponential (40-wide x 6 deep ≈ 4e9 nodes).
    # The shared budget must cap the total emitted rows regardless of depth/breadth,
    # proving the tree can never become a runaway payload.
    nested: object = 1
    for _ in range(6):
        nested = {f"k{i}": nested for i in range(40)}
    rows = introspect.namespace_rows({"v": nested})
    total = _count_rows(rows)
    # Real children are capped at _MAX_TOTAL_CHILDREN; elision markers (one per
    # over-breadth container) add at most as many again. Far below the exponential
    # blowup the budget prevents.
    assert total <= 2 * introspect._MAX_TOTAL_CHILDREN + 100
