# ruff: noqa: ANN401 -- introspect handles arbitrary Python objects; Any is the correct type throughout
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
from collections.abc import Iterator
from itertools import islice
from typing import Any

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
#
# Depth is generous so genuinely nested data (config trees, parsed JSON) expands
# all the way; what actually keeps the payload bounded is ``_MAX_TOTAL_CHILDREN``,
# a single ceiling on the *total* child rows emitted across the whole tree in one
# ``namespace_rows`` call. Without it, ``_BREADTH ** _MAX_DEPTH`` is astronomical;
# with it, deep nesting is free until the global budget runs out (then deeper
# branches simply stop expanding).
_MAX_DEPTH = 6
_BREADTH = 32
_MAX_TOTAL_CHILDREN = 2000

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
        except Exception:  # noqa: S112 -- intentional: skip unintrospectable values rather than failing
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


def binding_names(code: str) -> tuple[set[str], set[str]]:
    """Split a cell's identifiers into ``(assigned, used)`` by their syntactic role.

    ``assigned`` is every name the cell *binds*: a ``Store``/``Del`` ``Name`` (so
    plain assignment, augmented assignment, ``for`` targets, ``with as``,
    ``except as``, and the walrus all count), the bound head of each import, and
    each ``def``/``class`` name. ``used`` is every ``Load`` ``Name`` — a reference,
    read where the cell consumes the value.

    This is the kernel's own parse (the same walk :func:`cell_bindings` does),
    reused so a run's namespace references cost no extra runtime: attributing from
    the cell's *source* is also the only correct attribution when many background
    jobs mutate one shared namespace concurrently, where an after-the-fact
    namespace diff would credit one job's writes to another. A name that only
    appears inside a nested scope (a local in a ``def``) is conservatively
    included; that is harmless because references are only ever shown for names
    that are actually live top-level variables. Returns two empty sets for code
    that does not parse."""
    try:
        tree = ast.parse(code)
    except SyntaxError:
        return set(), set()
    assigned: set[str] = set()
    used: set[str] = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Name):
            if isinstance(node.ctx, (ast.Store, ast.Del)):
                assigned.add(node.id)
            else:
                used.add(node.id)
        elif isinstance(node, ast.alias):
            assigned.add(node.asname or node.name.split(".", 1)[0])
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            assigned.add(node.name)
        elif isinstance(node, ast.ExceptHandler) and node.name:
            # `except E as s` binds `s` as a bare string on the handler, not a
            # Store ``Name`` node, so ``ast.walk`` would otherwise miss it.
            assigned.add(node.name)
    return assigned, used


def unreached_bindings(code: str, fail_line: int) -> list[str]:
    """The cell-level names ``code`` provably never (re)bound in a run whose
    exception left the cell's top-level frame at ``fail_line``, in source order.

    The static half of the stale-binding trap (index#2526): a cell that raises
    partway leaves every later assignment unexecuted while the namespace keeps
    each name's old value, so a retry cell that reads one silently operates on
    stale state. This walk names those bindings so the runner can say so right
    in the traceback (``runtime._stale_binding_note``).

    Only provable claims are made. A statement binds its targets when it
    *completes*, so any binding statement ending at or after ``fail_line``
    never bound -- including the failing statement itself (its right side
    raised first). Bindings that happen mid-statement (a ``for``/``with``/
    ``except as`` target, a walrus) qualify only strictly after the failing
    line, and regions an interrupted run may still have executed are excluded
    entirely: the span of any loop enclosing the failing line (an earlier
    iteration ran its body) and the handlers/finally of any ``try`` enclosing
    it (they run during the unwind). Nested ``def``/``class``/lambda scopes
    bind locals, not cell names, so only the ``def``/``class`` name itself
    counts. Returns [] for code that does not parse."""
    try:
        tree = ast.parse(code)
    except SyntaxError:
        return []
    spans = _maybe_ran_spans(tree, fail_line)
    names: list[str] = []
    seen: set[str] = set()

    def emit(name: str, line: int) -> None:
        if name in seen or any(a <= line <= b for a, b in spans):
            return
        seen.add(name)
        names.append(name)

    def point(node: ast.AST) -> None:
        # A walrus binds the moment it evaluates; only one strictly after the
        # failing line is provably unexecuted (same-line order is unknowable
        # without column info).
        for name, line in _walrus_targets(node):
            if line > fail_line:
                emit(name, line)

    def visit(body: list[ast.stmt]) -> None:
        for stmt in body:
            end = stmt.end_lineno or stmt.lineno
            if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                # The name binds when the whole statement (decorators, bases,
                # defaults, class body) completes; the body's own assignments
                # bind locals or class attributes when called, never cell
                # names, so don't descend.
                if end >= fail_line:
                    emit(stmt.name, stmt.lineno)
            elif isinstance(stmt, (ast.For, ast.AsyncFor)):
                if stmt.lineno > fail_line:
                    for name, line in _name_targets(stmt.target):
                        emit(name, line)
                point(stmt.iter)
                visit(stmt.body)
                visit(stmt.orelse)
            elif isinstance(stmt, (ast.While, ast.If)):
                point(stmt.test)
                visit(stmt.body)
                visit(stmt.orelse)
            elif isinstance(stmt, (ast.With, ast.AsyncWith)):
                for item in stmt.items:
                    point(item.context_expr)
                    if item.optional_vars is not None and stmt.lineno > fail_line:
                        for name, line in _name_targets(item.optional_vars):
                            emit(name, line)
                visit(stmt.body)
            elif isinstance(stmt, (ast.Try, ast.TryStar)):
                visit(stmt.body)
                for handler in stmt.handlers:
                    if handler.name and handler.lineno > fail_line:
                        emit(handler.name, handler.lineno)
                    visit(handler.body)
                visit(stmt.orelse)
                visit(stmt.finalbody)
            elif isinstance(stmt, ast.Match):
                point(stmt.subject)
                for case in stmt.cases:
                    for name, line in _pattern_targets(case.pattern):
                        if line > fail_line:
                            emit(name, line)
                    if case.guard is not None:
                        point(case.guard)
                    visit(case.body)
            else:
                # A simple statement binds at completion: one that never
                # completed (it ends at/after the failing line) never bound.
                if end >= fail_line:
                    for name, line in _completion_targets(stmt):
                        emit(name, line)
                point(stmt)

    visit(tree.body)
    return names


def _maybe_ran_spans(tree: ast.Module, fail_line: int) -> list[tuple[int, int]]:
    """Line spans at/after ``fail_line`` that may nonetheless have executed,
    so :func:`unreached_bindings` must not claim anything inside them: the
    whole span of any loop enclosing the failing line (an earlier iteration
    ran its body) and the handlers/finally of any ``try`` enclosing it (a
    ``finally`` runs during the unwind; a handler may have run and re-raised)."""
    spans: list[tuple[int, int]] = []
    for node in ast.walk(tree):
        lineno = getattr(node, "lineno", None)
        end = getattr(node, "end_lineno", None)
        if lineno is None or end is None or not (lineno <= fail_line <= end):
            continue
        if isinstance(node, (ast.For, ast.AsyncFor, ast.While)):
            spans.append((lineno, end))
        elif isinstance(node, (ast.Try, ast.TryStar)):
            spans.extend((h.lineno, h.end_lineno or h.lineno) for h in node.handlers)
            if node.finalbody:
                last = node.finalbody[-1]
                spans.append((node.finalbody[0].lineno, last.end_lineno or last.lineno))
    return spans


def _completion_targets(stmt: ast.stmt) -> Iterator[tuple[str, int]]:
    """``(name, line)`` for each name a simple statement binds when it
    completes: assignment targets, import heads, ``del`` targets (a ``del``
    that never ran leaves the old value bound, the same stale trap)."""
    if isinstance(stmt, ast.Assign):
        for target in stmt.targets:
            yield from _name_targets(target)
    elif isinstance(stmt, ast.AugAssign) or (isinstance(stmt, ast.AnnAssign) and stmt.value is not None):
        yield from _name_targets(stmt.target)
    elif isinstance(stmt, ast.Delete):
        for target in stmt.targets:
            yield from _name_targets(target)
    elif isinstance(stmt, (ast.Import, ast.ImportFrom)):
        for alias in stmt.names:
            name = alias.asname or alias.name.split(".", 1)[0]
            if name != "*":
                yield name, alias.lineno


def _name_targets(target: ast.expr) -> Iterator[tuple[str, int]]:
    """``(name, line)`` for each plain name a target expression binds.
    Attribute and subscript targets mutate an existing object rather than bind
    a cell name, so they are skipped."""
    if isinstance(target, ast.Name):
        yield target.id, target.lineno
    elif isinstance(target, ast.Starred):
        yield from _name_targets(target.value)
    elif isinstance(target, (ast.Tuple, ast.List)):
        for elt in target.elts:
            yield from _name_targets(elt)


def _walrus_targets(node: ast.AST) -> Iterator[tuple[str, int]]:
    """``(name, line)`` for each walrus under ``node`` that binds the enclosing
    scope. Nested ``def``/``class``/lambda are skipped (their walruses bind
    locals); a comprehension's walrus deliberately counts (PEP 572 binds it
    outward), while its ``for`` targets are plain names, never ``NamedExpr``,
    so they need no special casing."""
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)):
        return
    if isinstance(node, ast.NamedExpr) and isinstance(node.target, ast.Name):
        yield node.target.id, node.target.lineno
    for child in ast.iter_child_nodes(node):
        yield from _walrus_targets(child)


def _pattern_targets(pattern: ast.pattern) -> Iterator[tuple[str, int]]:
    """``(name, line)`` for each capture a match pattern binds."""
    if isinstance(pattern, (ast.MatchAs, ast.MatchStar)):
        if pattern.name:
            yield pattern.name, pattern.lineno
    elif isinstance(pattern, ast.MatchMapping) and pattern.rest:
        yield pattern.rest, pattern.lineno
    for child in ast.iter_child_nodes(pattern):
        if isinstance(child, ast.pattern):
            yield from _pattern_targets(child)


def describe(value: Any) -> dict:
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
    refs: dict[str, dict] | None = None,
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
    fatal.

    ``refs`` (optional), keyed by name, attaches a variable's provenance to its
    *top-level* row: ``assigned_in`` and ``used_in``, each a list of run ids that
    bound or referenced the name (see :func:`binding_names`). Children never carry
    refs — provenance is a property of a variable, not of a container's members."""
    # One shared budget for the whole call: a mutable [remaining] cell decremented
    # as child rows are emitted, so the total tree (across every top-level name) is
    # bounded regardless of how deep or wide any one branch goes.
    budget = [_MAX_TOTAL_CHILDREN]
    rows: list[dict] = []
    for name, value in islice(ns.items(), max_names):
        row = _row(name, value, depth=0, max_depth=max_depth, breadth=breadth, budget=budget)
        if row is not None:
            if refs is not None and (ref := refs.get(name)):
                if ref.get("assigned_in"):
                    row["assigned_in"] = list(ref["assigned_in"])
                if ref.get("used_in"):
                    row["used_in"] = list(ref["used_in"])
            rows.append(row)
    # Only the top level sorts heaviest-first; children keep natural (dict/list)
    # order so the user sees the container's own layout when they drill in.
    rows.sort(key=lambda row: (-row["size"], row["name"]))
    return rows


def _row(name: str, value: Any, *, depth: int, max_depth: int, breadth: int, budget: list[int]) -> dict | None:
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
    # Descend only into expandable kinds, while we have depth budget left, and while
    # the global row budget is not yet spent. Children live one level deeper, so
    # stop once depth would reach max_depth.
    if kind in _EXPANDABLE_KINDS and depth < max_depth and budget[0] > 0:
        children = _children(value, kind, depth=depth + 1, max_depth=max_depth, breadth=breadth, budget=budget)
        if children:
            row["children"] = children
    return row


# The synthetic row appended when a container has more entries than ``breadth``:
# a single marker that says how many were elided, carrying no children of its own.
# Kept as ``object`` kind so the UI's existing chip vocabulary covers it.
def _elision_row(extra: int) -> dict:
    return {"name": "…", "type": "", "kind": "object", "repr": f"+{extra} more", "size": 0, "shape": ""}


def _children(value: Any, kind: str, *, depth: int, max_depth: int, breadth: int, budget: list[int]) -> list[dict]:
    """Rows for the entries of an expandable container, in natural order.

    Bounded to ``breadth`` real children plus at most one elision marker, and it
    never iterates more than ``breadth + 1`` items of any container (so a
    billion-entry dict costs the same as a small one). Decides what to expand from
    the live ``value``: a mapping yields ``key -> value`` rows, a sequence yields
    indexed (or, for sets, repr-named) element rows, and a plain object yields its
    public instance attributes. ``budget`` is the shared global row ceiling. Any
    failure degrades to no children, never a raise."""
    try:
        if isinstance(value, dict):
            return _mapping_children(value, depth=depth, max_depth=max_depth, breadth=breadth, budget=budget)
        if isinstance(value, (list, tuple)):
            return _sequence_children(value, depth=depth, max_depth=max_depth, breadth=breadth, budget=budget, indexed=True)
        if isinstance(value, (set, frozenset)):
            # Sets are unordered, so an index name would be meaningless; name each
            # child by its bounded element repr instead.
            return _sequence_children(value, depth=depth, max_depth=max_depth, breadth=breadth, budget=budget, indexed=False)
        if kind == "object":
            return _object_children(value, depth=depth, max_depth=max_depth, breadth=breadth, budget=budget)
    except Exception:
        # Introspecting the container itself misbehaved (a dict-like whose
        # iteration raises, a __len__ that lies): show it as a leaf, not a crash.
        return []
    return []


def _mapping_children(value: Any, *, depth: int, max_depth: int, breadth: int, budget: list[int]) -> list[dict]:
    """``key -> value`` rows for a dict, keyed by a short rendering of the key.

    ``islice`` over ``value.items()`` so we touch at most ``breadth + 1`` pairs of
    even an enormous dict; the extra pair is only probed to learn whether to emit
    the elision marker."""
    rows: list[dict] = []
    for k, v in islice(value.items(), breadth + 1):
        if budget[0] <= 0:
            break
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
        budget[0] -= 1
        child = _row(name, v, depth=depth, max_depth=max_depth, breadth=breadth, budget=budget)
        if child is not None:
            rows.append(child)
    return rows


def _sequence_children(value: Any, *, depth: int, max_depth: int, breadth: int, budget: list[int], indexed: bool) -> list[dict]:
    """Element rows for a list/tuple/set, ``indexed`` choosing ``"[i]"`` names
    (ordered sequences) versus the element repr (unordered sets)."""
    rows: list[dict] = []
    for i, v in enumerate(islice(value, breadth + 1)):
        if budget[0] <= 0:
            break
        if len(rows) == breadth:
            try:
                extra = len(value) - breadth
            except Exception:
                extra = 1
            rows.append(_elision_row(max(extra, 1)))
            break
        name = f"[{i}]" if indexed else _repr.repr(v)
        budget[0] -= 1
        child = _row(name, v, depth=depth, max_depth=max_depth, breadth=breadth, budget=budget)
        if child is not None:
            rows.append(child)
    return rows


def _object_children(value: Any, *, depth: int, max_depth: int, breadth: int, budget: list[int]) -> list[dict]:
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
    # Collect the *eligible* attributes (public, str-named, data not behavior)
    # first, then bound — so dunders and methods never consume a breadth slot nor
    # inflate the `+N more` count. Unlike a dict/list (which can be enormous, so we
    # cap iteration at breadth+1), an instance __dict__ is modest, so scanning it in
    # full to filter correctly is cheap.
    eligible: list[tuple[str, object]] = []
    for attr in members:
        if not isinstance(attr, str) or attr.startswith("__"):
            continue
        try:
            member = members[attr]
        except Exception:  # noqa: S112 -- intentional: skip attributes whose descriptor access raises
            continue
        # Skip methods/functions bound on the instance: they are behavior, not
        # the object's data, and clutter the browsable view.
        if callable(member) and isinstance(
            member, (types.FunctionType, types.MethodType, types.BuiltinFunctionType, type)
        ):
            continue
        eligible.append((attr, member))
    rows: list[dict] = []
    for attr, member in eligible[:breadth]:
        if budget[0] <= 0:
            break
        budget[0] -= 1
        child = _row(attr, member, depth=depth, max_depth=max_depth, breadth=breadth, budget=budget)
        if child is not None:
            rows.append(child)
    extra = len(eligible) - breadth
    if extra > 0:
        rows.append(_elision_row(extra))
    return rows


def _ns_shape(value: Any, kind: str) -> str:
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


def _describe_text(value: Any, type_name: str) -> dict:
    n = len(value)
    preview = _repr.repr(value)
    return {"kind": "text", "type": type_name, "summary": f"{preview} · {n}", "detail": preview}


def _describe_module(value: Any, type_name: str) -> dict:
    name = getattr(value, "__name__", "?")
    out = {"kind": "module", "type": type_name, "summary": f"module {name}", "detail": _doc_head(value)}
    location = _source_location(value)
    if location is not None:
        out["def"] = location
    return out


def _describe_callable(value: Any, type_name: str) -> dict:
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


def _describe_polars_df(value: Any, type_name: str) -> dict:
    rows, cols = value.height, value.width
    schema = _schema_lines(zip(value.columns, value.dtypes, strict=False), cols)
    return {
        "kind": "dataframe",
        "type": type_name,
        "summary": f"DataFrame {_compact_int(rows)}×{cols}",
        "detail": f"shape ({rows:,}, {cols})\n{schema}",
    }


def _describe_polars_lazy(value: Any, type_name: str) -> dict:
    detail = "LazyFrame (not yet collected)"
    try:
        schema = value.collect_schema()
        lines = _schema_lines(schema.items(), len(schema))
        detail = f"LazyFrame · {len(schema)} cols\n{lines}"
    except Exception:  # noqa: S110 -- schema resolution may fail for complex lazy plans; name-only fallback
        # An optimizer that cannot resolve the schema cheaply: name only.
        pass
    return {"kind": "lazyframe", "type": type_name, "summary": "LazyFrame", "detail": detail}


# Cap on schema lines in a frame's hover detail. A wide frame (thousands of
# feature columns) must not produce a multi-KB string that is stored per row and
# polled to the browser; show the head and a count of the rest.
_MAX_SCHEMA_COLS = 24


def _schema_lines(pairs: Any, total: int) -> str:
    """Up to ``_MAX_SCHEMA_COLS`` ``name: dtype`` lines from ``pairs``, with a
    ``… (+N more)`` tail when the frame is wider."""
    lines = [f"  {name}: {dtype}" for name, dtype in islice(pairs, _MAX_SCHEMA_COLS)]
    if total > _MAX_SCHEMA_COLS:
        lines.append(f"  … (+{total - _MAX_SCHEMA_COLS} more)")
    return "\n".join(lines)


def _describe_ndarray(value: Any, type_name: str) -> dict:
    shape = "×".join(str(d) for d in value.shape) or "scalar"
    dtype = str(value.dtype)
    return {
        "kind": "ndarray",
        "type": type_name,
        "summary": f"ndarray {dtype} ({shape})",
        "detail": f"dtype {dtype}, shape {tuple(value.shape)}, {value.size:,} elems",
    }


def _doc_head(value: Any) -> str:
    """The first line of an object's docstring, if any."""
    doc = inspect.getdoc(value)
    if not doc:
        return ""
    first = doc.strip().splitlines()[0]
    return first if len(first) <= 120 else first[:119] + "…"


def _source_location(value: Any) -> str | None:
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


def _looks_like_polars_df(value: Any) -> bool:
    return (
        type(value).__module__.split(".", 1)[0] == "polars"
        and hasattr(value, "height")
        and hasattr(value, "width")
        and hasattr(value, "columns")
        and hasattr(value, "dtypes")
    )


def _looks_like_polars_lazy(value: Any) -> bool:
    return (
        type(value).__module__.split(".", 1)[0] == "polars"
        and type(value).__name__ == "LazyFrame"
        and hasattr(value, "collect")
    )


def _looks_like_ndarray(value: Any) -> bool:
    return (
        type(value).__module__.split(".", 1)[0] == "numpy"
        and hasattr(value, "shape")
        and hasattr(value, "dtype")
        and hasattr(value, "size")
    )
