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
