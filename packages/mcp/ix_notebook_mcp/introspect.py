"""Describe the live values a cell's identifiers are bound to.

Runs *inside the kernel* (imported by :mod:`ix_notebook_mcp.runtime`), so it sees
the real objects in the user namespace, not a static guess. After a job finishes,
:func:`cell_bindings` walks the cell's source for the names it mentions, resolves
each one that exists in the namespace, and returns a compact descriptor per name.
The dashboard renders these as inlay hints (the ``summary``) and a hover card (the
``detail`` plus, for things with source, a ``def`` of ``file:line``).

This is a snapshot at execution time: an inlay shows the value as of the run that
produced it, which is the honest thing to show next to that run's code. It is
side-effect-free (only ``ns.get`` and metadata reads, never an eval of cell
expressions) and bounded (capped name count and capped reprs), so describing a
finished job is cheap and cannot perturb the namespace or flood the store.
"""

from __future__ import annotations

import ast
import inspect
import reprlib
import types
from itertools import islice

# A cell can mention many names; cap how many we resolve so a huge generated cell
# cannot blow up the per-row payload. The cell's own code bounds the realistic
# count well under this.
_MAX_NAMES = 64

# Bounded repr for previews: never walk a giant container or stringify megabytes.
_repr = reprlib.Repr()
_repr.maxstring = 120
_repr.maxother = 120
_repr.maxlist = 8
_repr.maxtuple = 8
_repr.maxset = 8
_repr.maxdict = 8
_repr.maxlevel = 2


def cell_bindings(code: str, ns: dict, *, max_names: int = _MAX_NAMES) -> dict[str, dict]:
    """Map each name the cell mentions that is live in ``ns`` to its descriptor.

    Keyed by name (not source position): the dashboard anchors every occurrence
    of a name and looks it up here, so one descriptor serves all uses. Returns an
    empty dict for code that does not parse."""
    try:
        names = _mentioned_names(code)
    except SyntaxError:
        return {}
    out: dict[str, dict] = {}
    for name in sorted(names):
        if name not in ns:
            continue
        try:
            out[name] = describe(ns[name])
        except Exception:
            # A value whose own introspection raises (an exotic __getattr__, a
            # property with side effects) simply contributes no hint.
            continue
        if len(out) >= max_names:
            break
    return out


def _mentioned_names(code: str) -> set[str]:
    """The top-level identifiers a cell binds or references: every ``Name``, the
    bound head of each import, and each def/class name. Attribute parts (the
    ``head`` in ``df.head``) are deliberately excluded, so an inlay attaches to
    the variable, not its methods."""
    tree = ast.parse(code)
    names: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Name):
            names.add(node.id)
        elif isinstance(node, ast.alias):
            # `import a.b as c` binds `c`; `import a.b` binds `a`.
            names.add(node.asname or node.name.split(".", 1)[0])
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            names.add(node.name)
    return names


def describe(value) -> dict:
    """A compact descriptor for one live value.

    Shape: ``{kind, type, summary, detail}`` plus an optional ``def`` of
    ``"file:line"`` when the value has locatable source (a function, class, or
    module). ``summary`` is the short inlay text; ``detail`` is the hover body."""
    type_name = type(value).__name__

    if value is None or isinstance(value, bool):
        return {"kind": "scalar", "type": type_name, "summary": repr(value), "detail": repr(value)}

    if isinstance(value, int):
        text = repr(value)
        summary = text if len(text) <= 24 else f"int ({len(text)} digits)"
        return {"kind": "scalar", "type": type_name, "summary": summary, "detail": text}

    if isinstance(value, float):
        return {"kind": "scalar", "type": type_name, "summary": repr(value), "detail": repr(value)}

    if isinstance(value, (str, bytes)):
        return _describe_text(value, type_name)

    if isinstance(value, types.ModuleType):
        return _describe_module(value, type_name)

    if callable(value) and isinstance(
        value, (types.FunctionType, types.MethodType, types.BuiltinFunctionType, type)
    ):
        return _describe_callable(value, type_name)

    if _looks_like_polars_df(value):
        return _describe_polars_df(value, type_name)

    if _looks_like_polars_lazy(value):
        return _describe_polars_lazy(value, type_name)

    if _looks_like_ndarray(value):
        return _describe_ndarray(value, type_name)

    if isinstance(value, dict):
        # islice, not list(value)[:6]: a huge dict must not be fully materialized
        # here, since this runs synchronously on the kernel's shared event loop.
        keys = ", ".join(_repr.repr(k) for k in islice(value, 6))
        more = ", …" if len(value) > 6 else ""
        return {
            "kind": "mapping",
            "type": type_name,
            "summary": f"{type_name}[{len(value)}]",
            "detail": f"{len(value)} keys: {keys}{more}" if value else "empty",
        }

    if isinstance(value, (list, tuple, set, frozenset)):
        return {
            "kind": "sequence",
            "type": type_name,
            "summary": f"{type_name}[{len(value)}]",
            "detail": _repr.repr(value),
        }

    return {"kind": "object", "type": type_name, "summary": type_name, "detail": _repr.repr(value)}


def _describe_text(value, type_name: str) -> dict:
    n = len(value)
    preview = _repr.repr(value)
    return {"kind": "text", "type": type_name, "summary": f"{preview} · {n}", "detail": preview}


def _describe_module(value, type_name: str) -> dict:
    name = getattr(value, "__name__", "?")
    out = {"kind": "module", "type": type_name, "summary": f"module {name}", "detail": _doc_head(value)}
    location = _source_location(value)
    if location is not None:
        out["def"] = location
    return out


def _describe_callable(value, type_name: str) -> dict:
    name = getattr(value, "__name__", type_name)
    is_class = isinstance(value, type)
    try:
        signature = str(inspect.signature(value))
    except (TypeError, ValueError):
        signature = "(…)"
    marker = "class" if is_class else "ƒ"
    summary = f"{marker} {name}{signature}"
    if len(summary) > 60:
        summary = summary[:59] + "…"
    detail_parts = [f"{name}{signature}"]
    doc = _doc_head(value)
    if doc:
        detail_parts.append(doc)
    out = {
        "kind": "class" if is_class else "callable",
        "type": type_name,
        "summary": summary,
        "detail": "\n".join(detail_parts),
    }
    location = _source_location(value)
    if location is not None:
        out["def"] = location
    return out


def _describe_polars_df(value, type_name: str) -> dict:
    rows, cols = value.height, value.width
    schema = _schema_lines(zip(value.columns, value.dtypes), cols)
    return {
        "kind": "dataframe",
        "type": type_name,
        "summary": f"DataFrame {_compact_int(rows)}×{cols}",
        "detail": f"shape ({rows:,}, {cols})\n{schema}",
    }


def _describe_polars_lazy(value, type_name: str) -> dict:
    detail = "LazyFrame (not yet collected)"
    try:
        schema = value.collect_schema()
        lines = _schema_lines(schema.items(), len(schema))
        detail = f"LazyFrame · {len(schema)} cols\n{lines}"
    except Exception:
        # An optimizer that cannot resolve the schema cheaply: name only.
        pass
    return {"kind": "lazyframe", "type": type_name, "summary": "LazyFrame", "detail": detail}


# Cap on schema lines in a frame's hover detail. A wide frame (thousands of
# feature columns) must not produce a multi-KB string that is stored per row and
# polled to the browser; show the head and a count of the rest.
_MAX_SCHEMA_COLS = 24


def _schema_lines(pairs, total: int) -> str:
    """Up to ``_MAX_SCHEMA_COLS`` ``name: dtype`` lines from ``pairs``, with a
    ``… (+N more)`` tail when the frame is wider."""
    lines = [f"  {name}: {dtype}" for name, dtype in islice(pairs, _MAX_SCHEMA_COLS)]
    if total > _MAX_SCHEMA_COLS:
        lines.append(f"  … (+{total - _MAX_SCHEMA_COLS} more)")
    return "\n".join(lines)


def _describe_ndarray(value, type_name: str) -> dict:
    shape = "×".join(str(d) for d in value.shape) or "scalar"
    dtype = str(value.dtype)
    return {
        "kind": "ndarray",
        "type": type_name,
        "summary": f"ndarray {dtype} ({shape})",
        "detail": f"dtype {dtype}, shape {tuple(value.shape)}, {value.size:,} elems",
    }


def _doc_head(value) -> str:
    """The first line of an object's docstring, if any."""
    doc = inspect.getdoc(value)
    if not doc:
        return ""
    first = doc.strip().splitlines()[0]
    return first if len(first) <= 120 else first[:119] + "…"


def _source_location(value) -> str | None:
    """``"file:line"`` for a value with on-disk source, else None.

    This is the go-to-definition payload: where the function, class, or module is
    actually defined. Returns None for C extensions, dynamically built objects,
    and anything ``inspect`` cannot locate (the honest signal that there is no
    place to jump to)."""
    try:
        file = inspect.getsourcefile(value)
    except TypeError:
        return None
    if not file:
        return None
    try:
        _, line = inspect.getsourcelines(value)
    except (OSError, TypeError):
        line = 0
    return f"{file}:{line}" if line else file


def _compact_int(n: int) -> str:
    """A human-scaled count for the inlay: ``1234`` -> ``1,234``, ``1200000`` ->
    ``1.2M``. Keeps the chip narrow without losing the magnitude."""
    if n < 10_000:
        return f"{n:,}"
    for limit, suffix in ((1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")):
        if n >= limit:
            scaled = n / limit
            return f"{scaled:.1f}{suffix}".replace(".0", "")
    return str(n)


def _looks_like_polars_df(value) -> bool:
    return (
        type(value).__module__.split(".", 1)[0] == "polars"
        and hasattr(value, "height")
        and hasattr(value, "width")
        and hasattr(value, "columns")
        and hasattr(value, "dtypes")
    )


def _looks_like_polars_lazy(value) -> bool:
    return (
        type(value).__module__.split(".", 1)[0] == "polars"
        and type(value).__name__ == "LazyFrame"
        and hasattr(value, "collect")
    )


def _looks_like_ndarray(value) -> bool:
    return (
        type(value).__module__.split(".", 1)[0] == "numpy"
        and hasattr(value, "shape")
        and hasattr(value, "dtype")
        and hasattr(value, "size")
    )
