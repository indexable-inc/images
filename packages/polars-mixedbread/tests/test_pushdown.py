"""Offline, deterministic checks for predicate pushdown.

Runnable as a plain script (``python tests/test_pushdown.py``) so the Nix build
can exercise it with only Polars and no built extension. It loads the pure
``_pushdown`` module by path (it never imports the ``polars_mixedbread`` package,
which would pull in the compiled cdylib): pass the module path in
``POLARS_MIXEDBREAD_PUSHDOWN`` or rely on the in-repo default.

The point of these tests is the safety invariant the reviewer caught us
violating once: the pushed-down filter must be a *superset* of the predicate, so
that re-applying the full predicate client-side (which can only remove rows)
yields exactly the predicate. The trap is negation over a partial subtree.
"""

from __future__ import annotations

import importlib.util
import os
import pathlib
import sys

import polars as pl

# Locate the pure `_pushdown` module: an explicit path (argv[1] or the
# POLARS_MIXEDBREAD_PUSHDOWN env var) wins, else fall back to the in-repo layout
# for a plain `python tests/test_pushdown.py` run from the package directory.
_DEFAULT = pathlib.Path(__file__).resolve().parent.parent / "python" / "polars_mixedbread" / "_pushdown.py"
_explicit = (sys.argv[1] if len(sys.argv) > 1 else None) or os.environ.get("POLARS_MIXEDBREAD_PUSHDOWN")
_MODULE_PATH = pathlib.Path(_explicit) if _explicit else _DEFAULT

_spec = importlib.util.spec_from_file_location("polars_mixedbread_pushdown", _MODULE_PATH)
assert _spec is not None and _spec.loader is not None, f"cannot load {_MODULE_PATH}"
_pushdown = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_pushdown)

PUSHABLE = {"source", "repo", "path", "title"}


def push(expr: pl.Expr) -> dict | None:
    return _pushdown.pushdown(expr, PUSHABLE)


def test_equality_pushes() -> None:
    assert push(pl.col("source") == "code") == {"key": "source", "operator": "eq", "value": "code"}
    assert push(pl.col("source") != "slack") == {
        "key": "source",
        "operator": "not_eq",
        "value": "slack",
    }


def test_literal_on_left_is_handled() -> None:
    # `"code" == col` should translate the same as `col == "code"`.
    assert push(pl.lit("code") == pl.col("source")) == {
        "key": "source",
        "operator": "eq",
        "value": "code",
    }


def test_and_keeps_pushable_conjuncts() -> None:
    both = (pl.col("source") == "code") & (pl.col("repo") == "ix")
    assert push(both) == {
        "all": [
            {"key": "source", "operator": "eq", "value": "code"},
            {"key": "repo", "operator": "eq", "value": "ix"},
        ]
    }
    # A non-pushable conjunct (a score range) only widens the server result, so
    # the pushable side is still sent.
    mixed = (pl.col("source") == "code") & (pl.col("score") > 0.9)
    assert push(mixed) == {"key": "source", "operator": "eq", "value": "code"}


def test_or_pushes_only_when_whole() -> None:
    whole = (pl.col("source") == "code") | (pl.col("source") == "slack")
    assert push(whole) == {
        "any": [
            {"key": "source", "operator": "eq", "value": "code"},
            {"key": "source", "operator": "eq", "value": "slack"},
        ]
    }
    # A partial Or would drop rows the predicate keeps, so it must not push.
    partial = (pl.col("source") == "code") | (pl.col("score") > 0.9)
    assert push(partial) is None


def test_not_uses_de_morgan() -> None:
    assert push(~(pl.col("source") == "code")) == {
        "key": "source",
        "operator": "not_eq",
        "value": "code",
    }
    # ~(A | B) -> ~A AND ~B
    assert push(~((pl.col("source") == "code") | (pl.col("repo") == "ix"))) == {
        "all": [
            {"key": "source", "operator": "not_eq", "value": "code"},
            {"key": "repo", "operator": "not_eq", "value": "ix"},
        ]
    }
    # ~(A & B) -> ~A OR ~B (both pushable here)
    assert push(~((pl.col("source") == "code") & (pl.col("repo") == "ix"))) == {
        "any": [
            {"key": "source", "operator": "not_eq", "value": "code"},
            {"key": "repo", "operator": "not_eq", "value": "ix"},
        ]
    }
    assert push(~~(pl.col("source") == "code")) == {
        "key": "source",
        "operator": "eq",
        "value": "code",
    }


def test_negation_of_partial_and_must_not_push() -> None:
    # The regression: ~((source==code) & (score>0.5)) == (source!=code) OR (score<=0.5).
    # The pushable side under negation becomes an Or branch (`source != code`),
    # but the score branch cannot push, so the whole Or must collapse to no
    # pushdown. Emitting `none[source==code]` here would tell the server to drop
    # every source==code row, silently losing the kept (source==code, score<=0.5)
    # rows that the client re-apply can never add back.
    expr = ~((pl.col("source") == "code") & (pl.col("score") > 0.5))
    assert push(expr) is None


def test_non_pushable_predicates_return_none() -> None:
    assert push(pl.col("score") > 0.9) is None  # not a metadata column
    assert push(pl.col("source").is_in(["code", "slack"])) is None  # opaque list literal
    assert push(pl.col("source").str.starts_with("co")) is None  # unsupported op
    assert push(pl.col("source") == pl.col("repo")) is None  # column == column, no literal
    assert push(pl.col("title") == "x") == {"key": "title", "operator": "eq", "value": "x"}


def test_non_string_literal_does_not_push() -> None:
    # A column declared/compared as non-string must not push (string-eq only),
    # so an int/float/bool literal is left to the client.
    assert push(pl.col("source") == 3) is None
    assert push(pl.col("source") == True) is None  # noqa: E712 - exercising a bool literal


def main() -> None:
    tests = [v for name, v in sorted(globals().items()) if name.startswith("test_") and callable(v)]
    for test in tests:
        test()
    print(f"ok: {len(tests)} pushdown tests passed")


if __name__ == "__main__":
    main()
