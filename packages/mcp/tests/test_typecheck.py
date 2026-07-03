"""Per-cell static type checking before execution (issue #1754, new feature).

Every ``python_exec`` cell is type-checked first; a type error blocks the cell
(it never runs) and the diagnostic is returned so the agent can fix and retry.
The hard constraint is zero false positives on the persistent namespace: prior-cell
names and injected helpers must not be flagged as undefined. These tests exercise
the checker directly (``typecheck.check``) and end to end through the runner.

They skip when ``ty`` is not resolvable, so a bare dev checkout without the nix
wrapper still collects them; the nix ``typecheckSmoke`` build provides ty.
"""

from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from ix_notebook_mcp import runtime, typecheck

_HAS_TY = typecheck._ty_bin() is not None
pytestmark = pytest.mark.skipif(not _HAS_TY, reason="ty not resolvable (IX_MCP_TY_BIN / PATH)")


def _check(code: str, namespace: dict | None = None) -> typecheck.TypeCheckResult:
    return asyncio.run(typecheck.check(code, namespace or {}))


# --------------------------------------------------------------------------- #
# the checker in isolation: clean passes, real errors block, no false positives
# --------------------------------------------------------------------------- #


def test_a_clean_cell_passes() -> None:
    assert _check("x = 1 + 2\ny = str(x)").ok


def test_a_type_error_is_caught() -> None:
    result = _check("n: int = 'not an int'")
    assert not result.ok
    assert "invalid-assignment" in result.report
    assert "line 1:" in result.report  # mapped back to the cell's own line


def test_an_undefined_name_is_caught() -> None:
    result = _check("print(totally_undefined_name)")
    assert not result.ok
    assert "unresolved-reference" in result.report


def test_injected_helpers_are_not_flagged_as_undefined() -> None:
    # sh/jobs/grep/Result/api are live objects, not in the cell source; stubbed as
    # Any, they must not trip unresolved-reference.
    ns = {"sh": object(), "jobs": {}, "grep": object(), "Result": runtime.Result, "api": object()}
    assert _check("out = await sh('echo hi')\nn = len(jobs)\nr = Result.ok('x')", ns).ok


def test_prior_cell_names_are_not_flagged() -> None:
    ns = {"prior_value": 42, "helper_fn": (lambda a: a)}
    assert _check("doubled = prior_value * 2\nresult = helper_fn(doubled)", ns).ok


def test_prior_scalar_still_catches_a_real_misuse() -> None:
    # A read-only prior int keeps its real type, so `.upper()` on it is caught --
    # the checking is real, not everything-is-Any.
    result = _check("prior_count.upper()", {"prior_count": 5})
    assert not result.ok
    assert "unresolved-attribute" in result.report


def test_reassigning_a_prior_name_to_a_new_type_is_allowed() -> None:
    # Python allows rebinding; a concrete stub would flag it. The reassigned name
    # degrades to Any so the legitimate rebind passes.
    assert _check("x = 'now a string'", {"x": 5}).ok


def test_top_level_await_and_yield_are_legal() -> None:
    assert _check("out = await sh('hi')", {"sh": object()}).ok
    assert _check("for i in range(3):\n    yield i").ok


def test_comprehensions_fstrings_and_imports_do_not_false_positive() -> None:
    assert _check("nums = [i * 2 for i in range(5)]\ntotal = sum(nums)").ok
    assert _check("name = 'x'\ng = f'hi {name} {1 + 2}'").ok
    assert _check("import os\np = os.path.join('a', 'b')").ok


def test_an_unparseable_cell_is_left_to_the_compile_path() -> None:
    # A SyntaxError is the real compile path's job to report; the checker must not
    # pre-empt it (returns ok so the runner surfaces the SyntaxError normally).
    assert _check("def broken(:\n    pass").ok


def test_a_star_import_cell_is_not_blocked() -> None:
    # `from x import *` is legal at the kernel's module scope but a SyntaxError
    # inside the `async def` wrapper (confirmed on ty 0.0.40: error[invalid-syntax]
    # on the cell's own line), so the checker skips such a cell instead of
    # blocking it.
    assert _check("from math import *\nx = sqrt(4)").ok


def test_deleting_a_prior_cell_name_is_not_flagged() -> None:
    # `del prior_name` unbinds the module-scope name; without a `global` in the
    # wrapper it would read as an unbound-local delete.
    assert _check("del prior_name", {"prior_name": 5}).ok


def test_a_nested_star_import_is_not_blocked() -> None:
    # `from x import *` under a top-level `if` is still module-scope at runtime
    # but a SyntaxError inside the wrapper; the skip must see nested statements.
    assert _check("if True:\n    from math import *").ok


def test_internal_helper_cells_are_not_flagged() -> None:
    # The read MCP tool submits `await __ix_read(...)` through the same runner
    # path; the runtime's __ix_* entrypoints live in the namespace and must be
    # stubbed (they are not Python-managed dunders), or every read() is blocked.
    assert _check("await __ix_read('x', None, None, session=None)", {"__ix_read": object()}).ok


def test_single_underscore_prior_names_are_stubbed() -> None:
    # `_df` is a real prior-cell binding, not an introspection artifact.
    assert _check("_out = _df", {"_df": object()}).ok


def test_a_prior_name_shadowing_a_builtin_uses_the_binding() -> None:
    # `id = 'abc'` in a prior cell shadows the builtin at runtime, so the stub
    # must shadow it for the check too: reading the str is fine, and calling it
    # like the builtin is now the real error it would be at runtime.
    assert _check("id.upper()", {"id": "abc"}).ok
    assert not _check("id(5)", {"id": "abc"}).ok


def test_a_user_future_import_is_not_blocked() -> None:
    # Legal at the cell's real module scope; a SyntaxError if indented into the
    # wrapper, so it is hoisted (blanked; the preamble already carries one).
    assert _check("from __future__ import annotations\nx: 'int' = 1").ok


def test_assignments_inside_compound_statements_bind_the_cell_scope() -> None:
    # `print(x)` then a conditional `x = 2`: the nested assignment binds the
    # cell's module scope, so the earlier read must resolve to the prior global
    # (missing the nested binding made x a wrapper-local, flagging the read).
    assert _check("print(prior)\nif True:\n    prior = 2", {"prior": 5}).ok


def test_annotating_a_prior_name_fails_open() -> None:
    # `print(x)` then `x: int = 2`: Python forbids `global` on a name annotated
    # in the same scope, so the wrapper cannot scope this shape correctly; the
    # checker skips the cell rather than flag the valid read.
    assert _check("print(prior)\nprior: int = 2", {"prior": 5}).ok


def test_workspace_modules_next_to_the_notebook_resolve(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # A first-party module in the kernel's working directory imports fine at
    # runtime (cwd is on sys.path), so the checker must resolve it too.
    (tmp_path / "myworkmod.py").write_text("VALUE: int = 7\n")
    monkeypatch.chdir(tmp_path)
    assert _check("import myworkmod\nprint(myworkmod.VALUE)").ok


def test_a_replayed_session_cell_is_never_blocked(monkeypatch: pytest.MonkeyPatch) -> None:
    # Session reopen re-runs already-successful cells (kind="replay"); blocking
    # one on a checker finding would silently drop its bindings from the
    # restored namespace.
    ns: dict = {"Result": runtime.Result}
    _wire(monkeypatch, ns)

    async def replay() -> runtime.Job:
        job = runtime.Job("replayed_flag = True\nbad: int = 'nope'", budget=5.0, kind="replay")
        runtime.jobs[job.id] = job
        job.task = asyncio.ensure_future(runtime._runner(job, ns))
        await job.task
        return job

    job = asyncio.run(replay())
    assert job.status == "done", (job.status, job.error)
    assert ns.get("replayed_flag") is True


# --------------------------------------------------------------------------- #
# the checker's own failure never blocks a cell
# --------------------------------------------------------------------------- #


def _install_hung_ty(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Point IX_MCP_TY_BIN at a stub that sleeps far past any test budget."""
    stub = tmp_path / "ty"
    stub.write_text("#!/bin/sh\nsleep 60\n")
    stub.chmod(0o755)
    monkeypatch.setenv("IX_MCP_TY_BIN", str(stub))


def test_a_hung_checker_reports_ok(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _install_hung_ty(tmp_path, monkeypatch)
    result = asyncio.run(typecheck.check("x = 1", {}, timeout=0.3))
    assert result.ok


def test_a_hung_checker_still_lets_the_cell_execute(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # The regression this guards: the timeout handler raising (e.g. the missing
    # contextlib import) propagated into _runner and recorded the cell as FAILED.
    # With a ty that hangs, the check must give up quietly and the cell must run.
    _install_hung_ty(tmp_path, monkeypatch)
    real_check = typecheck.check

    async def fast_check(code: str, namespace: dict) -> typecheck.TypeCheckResult:
        return await real_check(code, namespace, timeout=0.3)

    monkeypatch.setattr(typecheck, "check", fast_check)
    ns: dict = {"Result": runtime.Result}
    _wire(monkeypatch, ns)
    job = asyncio.run(runtime.__ix_run("ran_anyway = True\nResult.ok('ran')", budget=5.0))
    assert job.status == "done", (job.status, job.error)
    assert ns.get("ran_anyway") is True


# --------------------------------------------------------------------------- #
# end to end through the runner: a type error blocks execution
# --------------------------------------------------------------------------- #


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


def test_a_type_error_cell_is_blocked_before_it_executes(monkeypatch: pytest.MonkeyPatch) -> None:
    ns: dict = {"Result": runtime.Result}
    _wire(monkeypatch, ns)
    # The cell would set a side effect if it ran; it must not.
    job = asyncio.run(runtime.__ix_run("side_effect = 1\nbad: int = 'nope'", budget=5.0))
    assert job.status == "error"
    assert "Type check failed" in (job.error or "")
    assert "side_effect" not in ns, "the cell ran despite the type error"


def test_a_clean_cell_runs_through_the_runner(monkeypatch: pytest.MonkeyPatch) -> None:
    ns: dict = {"Result": runtime.Result}
    _wire(monkeypatch, ns)
    job = asyncio.run(runtime.__ix_run("value = 6 * 7\nResult.text(str(value))", budget=5.0))
    assert job.status == "done", (job.status, job.error)
    assert job.result.llm_result == "42"


def test_the_escape_hatch_disables_checking(monkeypatch: pytest.MonkeyPatch) -> None:
    ns: dict = {"Result": runtime.Result}
    _wire(monkeypatch, ns)
    monkeypatch.setenv("IX_MCP_TYPECHECK", "0")
    # A type error now runs (and simply fails or succeeds at runtime), rather than
    # being blocked by the checker. This cell is a valid assignment at runtime.
    job = asyncio.run(runtime.__ix_run("bad: int = 'nope'\nResult.ok('ran')", budget=5.0))
    assert job.status == "done", (job.status, job.error)
