"""Datalog over tree-sitter ASTs: query relations, scan lints, apply rewrites.

A rules program turns tree-sitter query matches into relations (one row per
match, columns named by ``@capture``), joins them with Datalog rules
(structurally via ``ancestor``/``parent``/``same-file``, by value via
``text``/``same-text``/``kind``, or recursively), and turns derived rows into
edits with ``(rewrite ...)`` templates or located findings with
``(lint ...)`` declarations::

    import astlog

    RULES = '''
    (rule (unwrap-call call e)
      (match rust "
        (call_expression
          function: (field_expression value: (_) @e field: (field_identifier) @m)
          arguments: (arguments)) @call")
      (text m "unwrap"))

    (rule (result-fn f)
      (match rust "
        (function_item return_type: (generic_type type: (type_identifier) @r)) @f")
      (text r "Result"))

    (rule (fixable call e)
      (unwrap-call call e)
      (result-fn f)
      (ancestor f call))

    (rewrite unwrap-to-try (fixable call e)
      (replace call "{e}?"))

    (lint fixable error "unwrap inside a Result-returning fn: `{e}`")
    '''

    relations = astlog.query(RULES, ["src/"])   # {relation: pl.DataFrame}
    findings = astlog.scan(RULES, ["src/"])     # pl.DataFrame of lint findings
    ignored = astlog.suppressed(RULES, ["src/"])  # what astlog-ignore hides + why
    edits = astlog.fixes(RULES, ["src/"])       # pl.DataFrame of planned edits
    print(astlog.fix(RULES, ["src/"]))          # unified diff (write=True applies)

Results come back as polars DataFrames, like every other bundled kernel
module. In ``query`` every relation column is a polars ``Struct`` with the
seven fields ``path``, ``kind``, ``start``, ``end``, ``line``, ``column``,
``text``: a node value fills them all, a derived text value carries only
``text`` (the rest are null). Directories are walked gitignore-aware; each
file's language is detected from its extension. The same engine backs the
``astlog`` CLI.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

import polars as pl

from ._astlog import __version__, fix
from ._astlog import fixes as _fixes
from ._astlog import query as _query
from ._astlog import scan as _scan
from ._astlog import suppressed as _suppressed

if TYPE_CHECKING:
    from ._astlog import Node

__all__ = ["__version__", "fix", "fixes", "query", "scan", "suppressed"]

# Every cell of a query relation column is normalized to these seven fields so
# the column is one well-typed polars Struct whether it holds nodes or derived
# text. Node cells fill all seven; text cells carry only `text`.
_NODE_FIELDS = ("path", "kind", "start", "end", "line", "column", "text")


def _value_struct() -> pl.Struct:
    """The Struct dtype every query relation column shares."""
    return pl.Struct(
        {
            "path": pl.Utf8,
            "kind": pl.Utf8,
            "start": pl.Int64,
            "end": pl.Int64,
            "line": pl.Int64,
            "column": pl.Int64,
            "text": pl.Utf8,
        }
    )


def _normalize_cell(value: Node | str) -> dict[str, object]:
    """A node dict passes through; a bare string becomes a text-only struct."""
    if isinstance(value, str):
        cell: dict[str, object] = {field: None for field in _NODE_FIELDS}
        cell["text"] = value
        return cell
    return dict(value)  # the cdylib already shaped node values as the seven fields


def query(
    rules: str,
    paths: list[str],
    relation: str | None = None,
) -> dict[str, pl.DataFrame]:
    """Evaluate ``rules`` over ``paths`` and return one DataFrame per relation.

    Each column is a polars ``Struct`` of the seven value fields; an empty
    relation keeps its columns and dtypes via the explicit schema.
    """
    out: dict[str, pl.DataFrame] = {}
    for name, relation_data in _query(rules, paths, relation).items():
        columns = relation_data["columns"]
        rows = relation_data["rows"]
        normalized = [{col: _normalize_cell(row[col]) for col in columns} for row in rows]
        out[name] = pl.DataFrame(
            normalized,
            schema={col: _value_struct() for col in columns},
        )
    return out


def scan(rules: str, paths: list[str]) -> pl.DataFrame:
    """Lint findings that survive ``astlog-ignore`` suppression, one per row."""
    return pl.DataFrame(
        _scan(rules, paths),
        schema={
            "file": pl.Utf8,
            "line": pl.Int64,
            "column": pl.Int64,
            "endLine": pl.Int64,
            "endColumn": pl.Int64,
            "rule": pl.Utf8,
            "severity": pl.Utf8,
            "message": pl.Utf8,
            "text": pl.Utf8,
        },
    )


def suppressed(rules: str, paths: list[str]) -> pl.DataFrame:
    """Findings an ``astlog-ignore`` comment hid, each with that comment.

    The scan columns plus ``commentLine``/``commentText``: an audit of what is
    explicitly ignored, where, and why.
    """
    return pl.DataFrame(
        _suppressed(rules, paths),
        schema={
            "file": pl.Utf8,
            "line": pl.Int64,
            "column": pl.Int64,
            "endLine": pl.Int64,
            "endColumn": pl.Int64,
            "rule": pl.Utf8,
            "severity": pl.Utf8,
            "message": pl.Utf8,
            "text": pl.Utf8,
            "commentLine": pl.Int64,
            "commentText": pl.Utf8,
        },
    )


def fixes(rules: str, paths: list[str]) -> pl.DataFrame:
    """Planned ``(rewrite ...)`` edits as byte-range replacements."""
    return pl.DataFrame(
        _fixes(rules, paths),
        schema={
            "path": pl.Utf8,
            "start": pl.Int64,
            "end": pl.Int64,
            "replacement": pl.Utf8,
        },
    )
