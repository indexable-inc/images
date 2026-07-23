"""Translate a Polars predicate into a Mixedbread metadata filter.

Kept separate from the package's ``__init__`` (which imports the compiled
extension) so this pure logic, the part most worth testing, can be exercised
with only Polars and no built cdylib (see ``tests/test_pushdown.py``).

The translation is best-effort: an untranslatable node contributes no
constraint, because the caller always re-applies the full predicate client-side.
The hard invariant: the emitted filter F must be a *superset* of the predicate P
(F keeps every row P keeps), since the client re-apply can only remove rows,
never add them back. Negation is the trap: complementing a superset yields a
*subset*, which would silently drop rows. So instead of emitting a ``none``
group, ``_walk`` threads polarity and pushes negation down to the leaves via De
Morgan (``And`` <-> ``Or``, ``eq`` <-> ``not_eq``). In a given polarity the rule
is then simple: the "any"-shaped node must have both sides pushable (dropping one
would narrow it), while the "all"-shaped node may drop unpushable sides (that
only widens it). A ``None`` child means "no constraint" (match everything),
always a safe superset.
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, TypeAlias

if TYPE_CHECKING:
    import polars as pl

# The Polars expression AST is `json.loads(expr.meta.serialize(...))`, i.e. an
# arbitrary parsed-JSON value. There is no published schema for the node shapes
# (it is a Polars-internal format), so the only honest static type is "any JSON
# value": a recursive union, walked with `isinstance` guards. This is precise --
# every access below narrows from it -- without claiming a structure Polars does
# not guarantee.
JSONValue: TypeAlias = (
    "str | int | float | bool | None | list[JSONValue] | dict[str, JSONValue]"
)

# The emitted Mixedbread metadata filter: either a leaf condition
# (`{"key","operator","value"}`, all string-valued) or an `all`/`any` group
# whose value is a list of nested filters. Both branches are `dict[str, ...]`,
# so the union of their value types (a string or a list of filters) is the
# value type of the returned dict.
Filter: TypeAlias = "dict[str, str | list[Filter]]"

# Polars comparison ops we can push to Mixedbread. Restricted to equality on its
# own: string equality is unambiguous, so a server-side `eq`/`not_eq` can never
# exclude a row the Polars predicate would keep (the failure mode that would lose
# data, since the client re-apply cannot add rows back). Range ops would risk a
# string-vs-number coercion mismatch server-side, so they stay client-side.
PUSHABLE_OPS = {"Eq": "eq", "NotEq": "not_eq"}


def pushdown(predicate: pl.Expr, pushable: set[str]) -> Filter | None:
    """Return a Mixedbread filter dict for the pushable part of ``predicate``.

    ``pushable`` is the set of (string-typed) metadata column names that map 1:1
    to Mixedbread metadata keys. Returns ``None`` when nothing pushes.
    """
    try:
        ast = json.loads(predicate.meta.serialize(format="json"))
        return _walk(ast, pushable, negated=False)
    except Exception:
        # The expression AST is a Polars-internal format; if it ever changes
        # shape (an unexpected node, a missing field), degrade to no pushdown
        # rather than break the query. Correctness still holds: the full
        # predicate is re-applied client-side regardless.
        return None


def _walk(node: JSONValue, pushable: set[str], *, negated: bool) -> Filter | None:
    if not isinstance(node, dict):
        return None
    binary = node.get("BinaryExpr")
    if isinstance(binary, dict):
        op = binary.get("op")
        if op in ("And", "Or"):
            # De Morgan: under negation And and Or swap.
            conjunctive = (op == "And") != negated
            left = _walk(binary.get("left"), pushable, negated=negated)
            right = _walk(binary.get("right"), pushable, negated=negated)
            if conjunctive:
                # "all": a dropped (None) side only widens the result, so keep
                # whatever pushed.
                parts = [p for p in (left, right) if p is not None]
                if not parts:
                    return None
                return parts[0] if len(parts) == 1 else {"all": list(parts)}
            # "any": dropping a side would narrow the result, so push only whole.
            if left is None or right is None:
                return None
            return {"any": [left, right]}
        if isinstance(op, str) and op in PUSHABLE_OPS:
            return _condition(binary, op, pushable, negated=negated)
        return None
    function = node.get("Function")
    if isinstance(function, dict) and function.get("function") == {"Boolean": "Not"}:
        inputs = function.get("input")
        if isinstance(inputs, list) and inputs:
            return _walk(inputs[0], pushable, negated=not negated)
    return None


def _condition(
    binary: dict[str, JSONValue], op: str, pushable: set[str], *, negated: bool
) -> Filter | None:
    """Map a `col == lit` / `col != lit` leaf to a Mixedbread condition.

    Under ``negated`` the operator is flipped (``eq`` <-> ``not_eq``), which is
    exact for equality, so a negated leaf carries no approximation up the tree.
    """
    column = _column_name(binary.get("left"))
    value = _string_literal(binary.get("right"))
    if column is None:  # Polars usually puts the column on the left, but allow lit == col.
        column = _column_name(binary.get("right"))
        value = _string_literal(binary.get("left"))
    if column not in pushable or value is None:
        return None
    operator = PUSHABLE_OPS[op]
    if negated:
        operator = "not_eq" if operator == "eq" else "eq"
    return {"key": column, "operator": operator, "value": value}


def _column_name(node: JSONValue) -> str | None:
    if isinstance(node, dict):
        column = node.get("Column")
        if isinstance(column, str):
            return column
    return None


def _string_literal(node: JSONValue) -> str | None:
    """The string value of a literal scalar node, or None if it is not a string."""
    if isinstance(node, dict):
        literal = node.get("Literal")
        scalar = literal.get("Scalar") if isinstance(literal, dict) else None
        if isinstance(scalar, dict):
            value = scalar.get("String")
            if isinstance(value, str):
                return value
    return None
