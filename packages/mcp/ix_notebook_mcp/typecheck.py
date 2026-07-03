"""Per-cell static type checking, run BEFORE a cell executes.

Every ``python_exec`` cell is type-checked first, so a type error is caught and
returned as the run result (with the checker's own diagnostic) instead of blowing
up at runtime three lines in. The checker is `ty` (astral-sh's Rust type checker):
a single fast binary, provided by the nix package (``IX_MCP_TY_BIN`` on the
wrapper's env, or ``ty`` on PATH), so nothing is fetched at runtime.

The hard part is the kernel's persistent namespace: names defined in earlier cells
and the injected helpers (``sh``, ``api``, ``jobs``, ``grep``, ``Result``, ...) are
all live objects, not something in the cell's source, so a naive check would flag
every one as an undefined name. The fix is to synthesize a tiny typed *preamble*
from the live namespace -- one declaration per name, its real builtin type where
that is cheap and safe, ``Any`` otherwise -- and prepend it to the cell before
checking. False positives that block a valid cell are worse than no checking, so
the preamble errs toward ``Any``: the worst case is a missed error, never a
spurious one.

The cell body is wrapped in ``async def __ix_cell__():`` (so top-level ``await``
and ``yield`` are legal, exactly as the real compile path allows) with a ``global``
declaration for every name it binds (so an assignment writes module scope and a
read resolves to the stubbed global, matching how the cell really runs). Line and
column numbers in the diagnostics are mapped back from the synthetic module to the
cell the caller wrote.
"""

from __future__ import annotations

import ast
import asyncio
import builtins
import contextlib
import keyword
import os
import pathlib
import re
import shutil
import signal
import sys
import tempfile
from collections.abc import Iterator
from dataclasses import dataclass

# A name is stubbed with its real type only for these simple, always-importable
# builtins; every other value (a module, a helper, an instance of some class) is
# stubbed as ``Any``. Real types here are what make prior-cell scalars actually
# check (``x = 5`` then ``x.upper()`` is caught); anything fancier risks a false
# positive, so it degrades to ``Any``.
_SIMPLE_SCALARS = (bool, int, float, complex, str, bytes)
_SIMPLE_CONTAINERS = {
    list: "list[Any]",
    dict: "dict[Any, Any]",
    tuple: "tuple[Any, ...]",
    set: "set[Any]",
    frozenset: "frozenset[Any]",
}

_BUILTIN_NAMES = frozenset(dir(builtins))

# `ty check` emits `path:line:col: severity[rule] message`. We only block on
# `error`-level diagnostics; warnings never fail a cell (a warning that blocked a
# valid cell would be exactly the false positive this feature must avoid).
_DIAG_RE = re.compile(r"^(?P<path>.*?):(?P<line>\d+):(?P<col>\d+): (?P<sev>\w+)\[(?P<rule>[\w-]+)\] (?P<msg>.*)$")


@dataclass(frozen=True)
class TypeCheckResult:
    """The outcome of checking one cell. ``ok`` is True when nothing blocks
    execution (no error-level diagnostics, or the checker was unavailable/skipped);
    ``report`` is the human/model-facing diagnostic text when ``ok`` is False."""

    ok: bool
    report: str = ""


def _stub_type(value: object) -> str:
    """The annotation to stub ``value`` with: its real builtin type where cheap
    and safe (an exact-type match on a simple scalar/container), else ``Any``.
    Anything unusual -- a subclass, an instance of some class, a module -- is
    ``Any``, which never produces a false positive."""
    tp = type(value)
    if tp in _SIMPLE_SCALARS:
        return tp.__name__
    return _SIMPLE_CONTAINERS.get(tp, "Any")


def _stubbable(name: str) -> bool:
    """Whether a live namespace ``name`` should get a preamble declaration.

    Everything a cell could legitimately read gets one: helper objects, user
    variables -- including single-underscore names (``_df`` is a real prior-cell
    binding) and names that shadow a builtin (``id = 'abc'``: the namespace
    binding is what the cell reads at runtime, so the stub must shadow the
    builtin for the check exactly as the binding shadows it at runtime). Skipped
    are non-identifiers, keywords, and Python-managed dunders (``__name__``,
    ``__builtins__``); the runtime's own ``__ix_*`` entrypoints are NOT dunders
    (no trailing underscores) and stay stubbable -- the ``read`` tool submits
    ``await __ix_read(...)`` cells that must resolve."""
    return (
        isinstance(name, str)
        and name.isidentifier()
        and not keyword.iskeyword(name)
        and not (name.startswith("__") and name.endswith("__"))
    )


def _scope_statements(body: list[ast.stmt]) -> Iterator[ast.stmt]:
    """Every statement that executes in the cell's own scope: the top level plus
    the bodies of top-level compound statements (``if``/``for``/``while``/
    ``with``/``try``/``match``), recursively -- but never the body of a
    ``def``/``class``, which is its own scope. An assignment inside a top-level
    ``if`` binds the cell's scope just like a bare one, so the wrapper's
    ``global``/stub bookkeeping must see it (missing it turned the name into a
    wrapper-local and flagged the earlier read as undefined)."""
    for node in body:
        yield node
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            continue  # its own scope; only its NAME binds the cell's scope
        for field in ("body", "orelse", "finalbody"):
            inner = getattr(node, field, None)
            if inner:
                yield from _scope_statements(inner)
        for handler in getattr(node, "handlers", []) or []:
            yield from _scope_statements(handler.body)
        for case in getattr(node, "cases", []) or []:
            yield from _scope_statements(case.body)


def _has_star_import(tree: ast.Module) -> bool:
    """True when any statement executing at the cell's scope is a
    ``from x import *`` -- including one nested in a top-level ``if``/``try``,
    which is legal at the kernel's module scope but a SyntaxError inside the
    ``async def`` wrapper."""
    return any(
        isinstance(node, ast.ImportFrom) and any(alias.name == "*" for alias in node.names)
        for node in _scope_statements(tree.body)
    )


def _assigned_names(tree: ast.Module) -> tuple[set[str], set[str]]:
    """The names a cell binds at its own scope, as ``(global_names, all_names)``.

    ``all_names`` is every binding -- assignment targets, ``for``/``with``
    bindings, ``def``/``class``/``import`` names, walrus targets, at the top
    level or inside a top-level compound statement -- and each gets a preamble
    declaration (so a brand-new name is defined rather than flagged, and a
    ``global`` has a binding to point at). ``global_names`` is the subset that
    gets a ``global`` in the wrapper (so the cell writes module scope, as it
    really does at the kernel's module level). It excludes annotated targets
    (``x: int = ...``): Python forbids ``global`` on a name annotated in the same
    scope, and an annotated binding already lands in the right place."""
    names: set[str] = set()
    annotated: set[str] = set()

    def add_target(target: ast.expr) -> None:
        if isinstance(target, ast.Name):
            names.add(target.id)
        elif isinstance(target, (ast.Tuple, ast.List)):
            for elt in target.elts:
                add_target(elt)
        elif isinstance(target, ast.Starred):
            add_target(target.value)

    def add_pattern(pattern: ast.pattern) -> None:
        # A `match` case pattern binds its capture names at the cell's scope.
        for sub in ast.walk(pattern):
            capture = getattr(sub, "name", None) or getattr(sub, "rest", None)
            if isinstance(capture, str):
                names.add(capture)

    for node in _scope_statements(tree.body):
        if isinstance(node, ast.Assign):
            for target in node.targets:
                add_target(target)
        elif isinstance(node, ast.AnnAssign):
            if isinstance(node.target, ast.Name):
                names.add(node.target.id)
                annotated.add(node.target.id)
        elif isinstance(node, (ast.AugAssign, ast.For, ast.AsyncFor)):
            add_target(node.target)
        elif isinstance(node, ast.Delete):
            # `del name` binds (well, unbinds) at the cell's scope: without a
            # `global`, the wrapper would treat a deleted prior-cell name as an
            # unbound local. Treat delete targets like assignments so a
            # namespaced one gets the `global` and a new one a stub.
            for target in node.targets:
                add_target(target)
        elif isinstance(node, (ast.With, ast.AsyncWith)):
            for item in node.items:
                if item.optional_vars is not None:
                    add_target(item.optional_vars)
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            names.add(node.name)
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            for alias in node.names:
                names.add((alias.asname or alias.name).split(".")[0])
        elif isinstance(node, ast.Match):
            for case in node.cases:
                add_pattern(case.pattern)
    # Walrus (:=) anywhere in the cell also binds at the enclosing scope.
    for sub in ast.walk(tree):
        if isinstance(sub, ast.NamedExpr) and isinstance(sub.target, ast.Name):
            names.add(sub.target.id)
    return names - annotated, names


def _preamble(namespace: dict, bound: set[str]) -> tuple[str, int]:
    """Build the typed preamble for ``namespace`` plus any ``bound`` cell-bound
    names not already in it. Returns ``(source, line_count)`` -- the line count is
    what the diagnostic line-mapping subtracts.

    A namespaced name the cell REASSIGNS is stubbed ``Any``, not its concrete
    type: the cell is about to rebind it (Python allows a new type), and a
    concrete stub would make ty flag that legitimate rebind. A name the cell only
    reads keeps its real type, so a prior-cell scalar still catches a genuine
    misuse (``x = 5`` in an earlier cell, then ``x.upper()`` here). ``from
    __future__ import annotations`` keeps the stub annotations lazy (a forward
    reference never has to resolve), and ``Any`` is imported once for the fallback.
    """
    lines = ["from __future__ import annotations", "from typing import Any"]
    declared: set[str] = set()
    for name, value in namespace.items():
        if not _stubbable(name):
            continue
        annotation = "Any" if name in bound else _stub_type(value)
        lines.append(f"{name}: {annotation}")
        declared.add(name)
    # A cell-local extra that shadows a builtin (`list = [...]` in THIS cell) gets
    # no declaration: it is a wrapper-local whose own assignment binds it, and a
    # module-level `list: Any` stub would blind the check to the real builtin for
    # every other use. (A namespaced shadowing name IS declared above: there the
    # live binding is what the cell reads, exactly as at runtime.)
    lines.extend(
        f"{name}: Any"
        for name in sorted(bound)
        if name not in declared and _stubbable(name) and name not in _BUILTIN_NAMES
    )
    body = "\n".join(lines) + "\n"
    return body, body.count("\n")


def _synthesize(code: str, namespace: dict) -> tuple[str, int] | None:
    """Turn a cell into a checkable synthetic module, or None to skip the check:
    either the cell does not parse (a SyntaxError is left for the real compile
    path to report, unchanged) or it cannot be represented in the wrapper without
    false positives (a top-level star-import).

    Returns ``(source, cell_line_offset)`` where a diagnostic on synthetic line L
    maps to cell line ``L - cell_line_offset``."""
    try:
        tree = ast.parse(code, "<cell>", "exec")
    except SyntaxError:
        return None
    # A `from x import *` at the cell's scope (top level, or nested in a
    # top-level `if`/`try`) is legal in the real cell (it executes at module
    # scope via PyCF_ALLOW_TOP_LEVEL_AWAIT) but a SyntaxError inside the
    # `async def` wrapper, so ty would flag a valid cell (confirmed on ty 0.0.40:
    # error[invalid-syntax] on the cell's own line). Star-imports also make the
    # bound-name set unknowable statically, so skip the check for such a cell
    # rather than block it.
    if _has_star_import(tree):
        return None
    global_names, all_bound = _assigned_names(tree)
    annotated = all_bound - global_names
    if annotated & namespace.keys():
        # The cell annotates a name that already lives in the namespace
        # (`print(x)` then `x: int = 2`): Python forbids `global` on a name
        # annotated in the same scope, so the wrapper cannot both resolve the
        # earlier read to the live global AND keep the annotation. Rather than
        # mis-scope it and flag a valid read, fail open for this (rare) shape.
        return None
    # Only names that ALREADY live in the namespace need a ``global`` -- those are
    # the prior-cell globals a read must resolve to and an assignment must write
    # back. A brand-new name stays a wrapper-local: harmless for the check, and it
    # sidesteps ty narrowing a global to its first literal type and then flagging a
    # legitimate same-cell rebind to another type (``y = 5`` then ``y = "s"``) --
    # which Python allows and must never block a cell.
    global_names = {n for n in global_names if n in namespace}
    preamble, preamble_lines = _preamble(namespace, all_bound)
    header = "async def __ix_cell__():\n"
    global_decl = f"    global {', '.join(sorted(global_names))}\n" if global_names else ""
    # A user `from __future__ import ...` is legal at the cell's real module scope
    # but a SyntaxError inside the wrapper; the preamble already opens with
    # `from __future__ import annotations`, so blank those lines (preserving the
    # line count for diagnostics) instead of indenting them into the function.
    lines = code.splitlines(keepends=True)
    for node in tree.body:
        if isinstance(node, ast.ImportFrom) and node.module == "__future__":
            for lineno in range(node.lineno, (node.end_lineno or node.lineno) + 1):
                lines[lineno - 1] = "\n"
    # Indent the cell verbatim (line count preserved 1:1; a constant column shift
    # of 4). Indenting inside a string literal only changes that literal's value,
    # never a type, so it cannot affect the check.
    indented = "".join("    " + line if line.strip() else line for line in lines)
    if indented and not indented.endswith("\n"):
        indented += "\n"
    source = preamble + header + global_decl + indented
    # Lines before the cell body: the preamble, the `async def` header (1), and
    # the optional `global` line (1). A diagnostic column subtracts the 4-space
    # indent.
    offset = preamble_lines + 1 + (1 if global_decl else 0)
    return source, offset


def _kill(proc: asyncio.subprocess.Process) -> None:
    """SIGKILL the checker's whole process group (best-effort; a race where it
    already exited is ignored). ``start_new_session`` made it a group leader."""
    if proc.returncode is not None:
        return
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(proc.pid, signal.SIGKILL)


def _ty_bin() -> str | None:
    """The ty binary: ``IX_MCP_TY_BIN`` (set on the nix wrapper) or ``ty`` on PATH.
    None when neither resolves, so the checker degrades to a no-op rather than
    erroring."""
    explicit = os.environ.get("IX_MCP_TY_BIN")
    if explicit and pathlib.Path(explicit).exists():
        return explicit
    return shutil.which("ty")


def _remap(output: str, synthetic_path: pathlib.Path, offset: int) -> tuple[str, bool]:
    """Rewrite ty's diagnostics to reference cell lines, and report whether any is
    an ``error`` (only errors block). Non-diagnostic lines (the ``Found N
    diagnostics`` footer, blank lines) are dropped so the report is just the
    findings the agent must fix.

    ty prints the diagnostic path RELATIVE to its own cwd when the file sits
    under it (the common case: ty runs with ``cwd=`` the temp dir holding
    ``cell.py``) and absolute otherwise (observed when the temp dir is reached
    through a symlinked prefix, macOS ``/var`` -> ``/private/var``, so the
    prefixes do not string-match). A relative path must resolve against the
    synthetic file's own directory: resolving it against the *kernel process's*
    cwd silently dropped every finding inside the nix build sandbox, whose cwd
    has no symlink prefix -- which made the whole gate pass vacuously there."""
    target = synthetic_path.resolve()
    findings: list[str] = []
    had_error = False
    for raw in output.splitlines():
        m = _DIAG_RE.match(raw)
        if m is None:
            continue
        diag = pathlib.Path(m.group("path"))
        if not diag.is_absolute():
            diag = synthetic_path.parent / diag
        if diag.resolve() != target:
            continue
        cell_line = int(m.group("line")) - offset
        # A diagnostic on the preamble/wrapper (cell_line < 1) is an artifact of
        # the synthesis, not the user's code; drop it rather than point at a line
        # they cannot see.
        if cell_line < 1:
            continue
        cell_col = max(int(m.group("col")) - 4, 1)
        sev = m.group("sev")
        if sev == "error":
            had_error = True
        findings.append(f"line {cell_line}:{cell_col}: {sev}[{m.group('rule')}] {m.group('msg')}")
    return "\n".join(findings), had_error


async def check(code: str, namespace: dict, *, timeout: float = 10.0) -> TypeCheckResult:
    """Type-check ``code`` against the live ``namespace``. Returns an ``ok`` result
    when nothing blocks (clean, checker unavailable, unparseable cell, or the
    checker itself failed to run) -- the feature never turns its own failure into a
    blocked cell. A blocking result carries the remapped diagnostic in ``report``.

    Known race, accepted: ``namespace`` is the live dict, so a concurrent cell
    rebinding a shared name to a different type mid-check can make its stub stale
    and flag (or miss) one run spuriously. The exact-type allowlist bounds the
    blast radius -- only simple builtin scalars/containers ever get a concrete
    stub, everything else is ``Any`` -- and per-session namespaces (the default on
    the HTTP transport) keep parallel agents out of each other's names, so a
    snapshot/lock here would buy little for its cost.
    """
    ty = _ty_bin()
    if ty is None:
        return TypeCheckResult(ok=True)
    synthesized = _synthesize(code, namespace)
    if synthesized is None:
        # Unparseable (the real compile path reports the SyntaxError) or
        # unrepresentable in the wrapper (a star-import): run the cell unchecked.
        return TypeCheckResult(ok=True)
    source, offset = synthesized
    with tempfile.TemporaryDirectory(prefix="ix-typecheck-") as tmp:
        path = pathlib.Path(tmp) / "cell.py"
        path.write_text(source, encoding="utf-8")
        argv = [
            ty,
            "check",
            # Resolve third-party imports against the kernel's own interpreter, so
            # a cell importing a bundled module (polars, httpx, ...) checks with
            # that module's real types rather than tripping unresolved-import.
            "--python",
            os.environ.get("IX_MCP_TY_PYTHON", sys.executable),
            # The synthetic module lives in a private temp dir, but the CELL runs
            # with the kernel's working directory on sys.path: a first-party
            # module sitting next to the notebook (`import mymodule`) resolves at
            # runtime, so it must resolve for the checker too or a valid import
            # would be flagged unresolved.
            "--extra-search-path",
            str(pathlib.Path.cwd()),
            "--output-format",
            "concise",
            "--no-progress",
            "--color",
            "never",
            str(path),
        ]
        try:
            proc = await asyncio.create_subprocess_exec(
                *argv,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
                cwd=tmp,
                start_new_session=True,  # own process group: a kill reaps ty and any child
            )
        except OSError:
            # The checker could not be spawned: never block a cell on our own
            # tooling failing to start.
            return TypeCheckResult(ok=True)
        try:
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout)
        except TimeoutError:
            # The checker hung past its own budget: kill it and let the cell run
            # (its own failures never block a cell).
            _kill(proc)
            with contextlib.suppress(TimeoutError):
                await asyncio.wait_for(proc.wait(), 2.0)
            return TypeCheckResult(ok=True)
        except asyncio.CancelledError:
            # The awaiting cell was cancelled mid-check: take ty down with it so a
            # cancelled cell never leaves an orphaned checker running. No await
            # here (the cancellation would be re-delivered at the next await);
            # the loop's child watcher reaps the killed process.
            _kill(proc)
            raise
        report, had_error = _remap(stdout.decode("utf-8", "replace"), path, offset)
        if not had_error:
            return TypeCheckResult(ok=True)
        header = (
            "Type check failed (ty) -- the cell was not run. Fix the type error and "
            "retry, or set IX_MCP_TYPECHECK=0 to disable per-cell checking:\n"
        )
        return TypeCheckResult(ok=False, report=header + report)
