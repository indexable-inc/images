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
import keyword
import os
import pathlib
import re
import shutil
import sys
import tempfile
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


def _stub_type(value: Any) -> str:
    """The annotation to stub ``value`` with: its real builtin type where cheap
    and safe (an exact-type match on a simple scalar/container), else ``Any``.
    Anything unusual -- a subclass, an instance of some class, a module -- is
    ``Any``, which never produces a false positive."""
    tp = type(value)
    if tp in _SIMPLE_SCALARS:
        return tp.__name__
    return _SIMPLE_CONTAINERS.get(tp, "Any")


def _stubbable(name: str) -> bool:
    """Whether ``name`` should get a preamble declaration. Skip dunders and
    private introspection names, non-identifiers, keywords, and anything that
    shadows a builtin (``list``, ``print``): ty already knows the builtins, and
    redeclaring one as ``Any`` would blind the check to it."""
    return (
        name.isidentifier()
        and not keyword.iskeyword(name)
        and not name.startswith("_")
        and name not in _BUILTIN_NAMES
    )


def _assigned_names(tree: ast.Module) -> tuple[set[str], set[str]]:
    """The top-level names a cell binds, as ``(global_names, all_names)``.

    ``all_names`` is every binding -- assignment targets, ``for``/``with``
    bindings, ``def``/``class``/``import`` names, walrus targets -- and each gets
    a preamble declaration (so a brand-new name is defined rather than flagged,
    and a ``global`` has a binding to point at). ``global_names`` is the subset
    that gets a ``global`` in the wrapper (so the cell writes module scope, as it
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

    for node in tree.body:
        if isinstance(node, ast.Assign):
            for target in node.targets:
                add_target(target)
        elif isinstance(node, ast.AnnAssign):
            if isinstance(node.target, ast.Name):
                names.add(node.target.id)
                annotated.add(node.target.id)
        elif isinstance(node, (ast.AugAssign, ast.For, ast.AsyncFor)):
            add_target(node.target)
        elif isinstance(node, (ast.With, ast.AsyncWith)):
            for item in node.items:
                if item.optional_vars is not None:
                    add_target(item.optional_vars)
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            names.add(node.name)
        elif isinstance(node, (ast.Import, ast.ImportFrom)):
            for alias in node.names:
                names.add((alias.asname or alias.name).split(".")[0])
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
    lines.extend(
        f"{name}: Any" for name in sorted(bound) if name not in declared and _stubbable(name)
    )
    body = "\n".join(lines) + "\n"
    return body, body.count("\n")


def _synthesize(code: str, namespace: dict) -> tuple[str, int] | None:
    """Turn a cell into a checkable synthetic module, or None if the cell does not
    parse (a SyntaxError is left for the real compile path to report, unchanged).

    Returns ``(source, cell_line_offset)`` where a diagnostic on synthetic line L
    maps to cell line ``L - cell_line_offset``."""
    try:
        tree = ast.parse(code, "<cell>", "exec")
    except SyntaxError:
        return None
    global_names, all_bound = _assigned_names(tree)
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
    # Indent the cell verbatim (line count preserved 1:1; a constant column shift
    # of 4). Indenting inside a string literal only changes that literal's value,
    # never a type, so it cannot affect the check.
    indented = "".join("    " + line if line.strip() else line for line in code.splitlines(keepends=True))
    if indented and not indented.endswith("\n"):
        indented += "\n"
    source = preamble + header + global_decl + indented
    # Lines before the cell body: the preamble, the `async def` header (1), and
    # the optional `global` line (1). A diagnostic column subtracts the 4-space
    # indent.
    offset = preamble_lines + 1 + (1 if global_decl else 0)
    return source, offset


def _ty_bin() -> str | None:
    """The ty binary: ``IX_MCP_TY_BIN`` (set on the nix wrapper) or ``ty`` on PATH.
    None when neither resolves, so the checker degrades to a no-op rather than
    erroring."""
    explicit = os.environ.get("IX_MCP_TY_BIN")
    if explicit and pathlib.Path(explicit).exists():
        return explicit
    return shutil.which("ty")


def _remap(output: str, synthetic_path: str, offset: int) -> tuple[str, bool]:
    """Rewrite ty's diagnostics to reference cell lines, and report whether any is
    an ``error`` (only errors block). Non-diagnostic lines (the ``Found N
    diagnostics`` footer, blank lines) are dropped so the report is just the
    findings the agent must fix."""
    findings: list[str] = []
    had_error = False
    for raw in output.splitlines():
        m = _DIAG_RE.match(raw)
        if m is None:
            continue
        if str(pathlib.Path(m.group("path")).resolve()) != synthetic_path:
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
    """
    ty = _ty_bin()
    if ty is None:
        return TypeCheckResult(ok=True)
    synthesized = _synthesize(code, namespace)
    if synthesized is None:
        # Unparseable: let the real compile path report the SyntaxError.
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
            with contextlib.suppress(ProcessLookupError):
                proc.kill()
            with contextlib.suppress(Exception):
                await proc.wait()
            return TypeCheckResult(ok=True)
        report, had_error = _remap(stdout.decode("utf-8", "replace"), str(path.resolve()), offset)
        if not had_error:
            return TypeCheckResult(ok=True)
        header = (
            "Type check failed (ty) -- the cell was not run. Fix the type error and "
            "retry, or set IX_MCP_TYPECHECK=0 to disable per-cell checking:\n"
        )
        return TypeCheckResult(ok=False, report=header + report)
