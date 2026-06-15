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
import sys
import types
from itertools import islice

# A cell can mention many names; cap how many we resolve so a huge generated cell
# cannot blow up the per-row payload. The cell's own code bounds the realistic
# count well under this.
_MAX_NAMES = 64

# How many children to emit per expanded container, and how many levels deep to
# descend. Both bound the tree the dashboard's namespace view can browse: a row at
# depth 0 may carry children down to (but not past) ``_MAX_DEPTH``, and any one
# container contributes at most ``_BREADTH`` real children plus a single elision
# marker. Mirrors the spirit of ``_MAX_NAMES`` — a session with a million-entry
# dict must not turn one job-finish into a million-row payload.
_MAX_DEPTH = 2
_BREADTH = 32

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


# Map a `describe` kind onto the short kind the dashboard's namespace renderer
# chips on. Most pass through unchanged; the frame/array/function aliases keep the
# UI vocabulary small (one chip per family).
_NS_KIND = {
    "dataframe": "frame",
    "lazyframe": "frame",
    "ndarray": "array",
    "callable": "function",
}


# Kinds with no meaningful in-memory footprint to a user: a module/function/class
# is shared machinery, not data the session "holds". Reported as size 0 so they
# sort below real data and show no size chip.
_SIZELESS_KINDS = frozenset({"module", "function", "class"})

# The short kinds whose *value* is worth drilling into. A mapping/sequence/object
# holds browsable children; everything else (module, class, function, scalar,
# text, frame, array) carries its weight in its own ``shape``/``repr``, so the UI
# gets nothing useful from expanding it. We decide expandability from the live
# value (see :func:`_children`), not the string alone, but this gates which kinds
# are even candidates.
_EXPANDABLE_KINDS = frozenset({"mapping", "sequence", "object"})


def namespace_rows(
    ns: dict,
    *,
    max_names: int = _MAX_NAMES,
    max_depth: int = _MAX_DEPTH,
    breadth: int = _BREADTH,
) -> list[dict]:
    """Describe values in ``ns`` as namespace-view rows.

    ``ns`` is the already-filtered set of user names (the caller drops baseline
    helpers, dunders, and history machinery). Each row is
    ``{name, type, kind, repr, size, shape}``: ``kind`` drives the chip, ``repr``
    a one-line preview (empty for frames/arrays, which describe themselves by
    ``shape``), ``size`` the shallow byte size (``getsizeof``, O(1) per name), and
    ``shape`` the dims for arrays/frames. An expandable container (mapping,
    sequence, or plain object) also carries ``children``: the same row shape for
    its entries, recursively, so the view can drill in. Sorted heaviest-first so
    the eye lands on what holds the memory. Capped at ``max_names`` (like
    :func:`cell_bindings`) so a session with thousands of globals cannot stall the
    kernel's event loop on a job finish. Reuses :func:`describe`, so it is bounded
    and side-effect-free; a value whose introspection raises is skipped, not
    fatal."""
    rows: list[dict] = []
    for name, value in islice(ns.items(), max_names):
        row = _row(name, value, depth=0, max_depth=max_depth, breadth=breadth)
        if row is not None:
            rows.append(row)
    # Only the top level sorts heaviest-first; children keep natural (dict/list)
    # order so the user sees the container's own layout when they drill in.
    rows.sort(key=lambda row: (-row["size"], row["name"]))
    return rows


def _row(name, value, *, depth: int, max_depth: int, breadth: int) -> dict | None:
    """Build one namespace row for ``name``/``value``, the single construction
    path shared by the top level and the recursion.

    Returns None only when the value's own introspection raises (the caller drops
    it — at the top level a name disappears, as a child it is simply absent), so a
    pathological value never propagates an exception out of :func:`namespace_rows`.
    ``depth`` is the row's level (top-level is 0); children are emitted only while
    ``depth < max_depth``."""
    try:
        described = describe(value)
    except Exception:
        # A value whose introspection raises (an exotic __getattr__, a property
        # with side effects) contributes no row rather than crashing the walk.
        return None
    kind = _NS_KIND.get(described["kind"], described["kind"])
    if kind in _SIZELESS_KINDS:
        size = 0
    else:
        try:
            size = int(sys.getsizeof(value))
        except Exception:
            size = 0
    row = {
        "name": name,
        "type": described["type"],
        "kind": kind,
        # Frames/arrays carry their weight in `shape`; everything else shows the
        # `describe` summary as a one-line preview.
        "repr": "" if kind in ("frame", "array") else described.get("summary", ""),
        "size": size,
        "shape": _ns_shape(value, kind),
    }
    # Descend only into expandable kinds, and only while we have depth budget left.
    # Children live one level deeper, so stop once depth would reach max_depth.
    if kind in _EXPANDABLE_KINDS and depth < max_depth:
        children = _children(value, kind, depth=depth + 1, max_depth=max_depth, breadth=breadth)
        if children:
            row["children"] = children
    return row


# The synthetic row appended when a container has more entries than ``breadth``:
# a single marker that says how many were elided, carrying no children of its own.
# Kept as ``object`` kind so the UI's existing chip vocabulary covers it.
def _elision_row(extra: int) -> dict:
    return {"name": "…", "type": "", "kind": "object", "repr": f"+{extra} more", "size": 0, "shape": ""}


def _children(value, kind: str, *, depth: int, max_depth: int, breadth: int) -> list[dict]:
    """Rows for the entries of an expandable container, in natural order.

    Bounded to ``breadth`` real children plus at most one elision marker, and it
    never iterates more than ``breadth + 1`` items of any container (so a
    billion-entry dict costs the same as a small one). Decides what to expand from
    the live ``value``: a mapping yields ``key -> value`` rows, a sequence yields
    indexed (or, for sets, repr-named) element rows, and a plain object yields its
    public instance attributes. Any failure degrades to no children, never a
    raise."""
    try:
        if isinstance(value, dict):
            return _mapping_children(value, depth=depth, max_depth=max_depth, breadth=breadth)
        if isinstance(value, (list, tuple)):
            return _sequence_children(value, depth=depth, max_depth=max_depth, breadth=breadth, indexed=True)
        if isinstance(value, (set, frozenset)):
            # Sets are unordered, so an index name would be meaningless; name each
            # child by its bounded element repr instead.
            return _sequence_children(value, depth=depth, max_depth=max_depth, breadth=breadth, indexed=False)
        if kind == "object":
            return _object_children(value, depth=depth, max_depth=max_depth, breadth=breadth)
    except Exception:
        # Introspecting the container itself misbehaved (a dict-like whose
        # iteration raises, a __len__ that lies): show it as a leaf, not a crash.
        return []
    return []


def _mapping_children(value, *, depth: int, max_depth: int, breadth: int) -> list[dict]:
    """``key -> value`` rows for a dict, keyed by a short rendering of the key.

    ``islice`` over ``value.items()`` so we touch at most ``breadth + 1`` pairs of
    even an enormous dict; the extra pair is only probed to learn whether to emit
    the elision marker."""
    rows: list[dict] = []
    for k, v in islice(value.items(), breadth + 1):
        if len(rows) == breadth:
            # We pulled one entry past the cap purely to detect overflow. Count
            # the remainder via len() (O(1)) rather than walking the rest.
            try:
                extra = len(value) - breadth
            except Exception:
                extra = 1
            rows.append(_elision_row(max(extra, 1)))
            break
        # str keys read cleanest unquoted-but-short; other keys go through the
        # bounded repr so a giant key cannot blow up the child name.
        name = k if isinstance(k, str) and len(k) <= 60 else _repr.repr(k)
        child = _row(name, v, depth=depth, max_depth=max_depth, breadth=breadth)
        if child is not None:
            rows.append(child)
    return rows


def _sequence_children(value, *, depth: int, max_depth: int, breadth: int, indexed: bool) -> list[dict]:
    """Element rows for a list/tuple/set, ``indexed`` choosing ``"[i]"`` names
    (ordered sequences) versus the element repr (unordered sets)."""
    rows: list[dict] = []
    for i, v in enumerate(islice(value, breadth + 1)):
        if len(rows) == breadth:
            try:
                extra = len(value) - breadth
            except Exception:
                extra = 1
            rows.append(_elision_row(max(extra, 1)))
            break
        name = f"[{i}]" if indexed else _repr.repr(v)
        child = _row(name, v, depth=depth, max_depth=max_depth, breadth=breadth)
        if child is not None:
            rows.append(child)
    return rows


def _object_children(value, *, depth: int, max_depth: int, breadth: int) -> list[dict]:
    """Rows for a plain object's public instance attributes.

    Conservative on purpose: only the instance ``__dict__`` (via ``vars``), never
    class attributes, properties, or methods — those can run arbitrary code or
    recurse forever. Dunders and callables are skipped, and every attribute read
    is guarded because a value sitting in ``__dict__`` is inert but reading it back
    through ``getattr`` could still hit a descriptor that raises."""
    try:
        members = vars(value)
    except TypeError:
        # No instance __dict__ (slots-only, or a C type): nothing browsable.
        return []
    rows: list[dict] = []
    for attr in islice(members, breadth + 1):
        if not isinstance(attr, str) or attr.startswith("__"):
            continue
        if len(rows) == breadth:
            rows.append(_elision_row(max(len(members) - breadth, 1)))
            break
        try:
            member = members[attr]
        except Exception:
            continue
        # Skip methods/functions bound on the instance: they are behavior, not
        # the object's data, and clutter the browsable view.
        if callable(member) and isinstance(
            member, (types.FunctionType, types.MethodType, types.BuiltinFunctionType, type)
        ):
            continue
        child = _row(attr, member, depth=depth, max_depth=max_depth, breadth=breadth)
        if child is not None:
            rows.append(child)
    return rows


def _ns_shape(value, kind: str) -> str:
    """Dims for an array (``50000×784``) or frame (``rows×cols``), else empty."""
    if kind == "array":
        try:
            return "×".join(str(int(dim)) for dim in value.shape)
        except Exception:
            return ""
    if kind == "frame":
        try:
            return f"{int(value.height)}×{int(value.width)}"
        except Exception:
            return ""
    return ""


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
