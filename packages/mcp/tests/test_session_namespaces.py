"""Per-session namespaces and Result ergonomics (ENG-2689).

One kernel serves every MCP client of the HTTP transport, and a single shared
namespace lets parallel agents clobber each other's variables. Each session id
now keys its own module-level globals dict, seeded from the shared read-only
helper area, while the no-session path keeps today's single shared namespace.
Cell results follow Jupyter semantics: the last expression is the result
whatever its type, stdout rides along with it, and a None-valued last statement
(an assignment, a bare print(), a side-effecting call) returns the captured
stdout or a quiet ok.
"""

from __future__ import annotations

import asyncio
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

from ix_notebook_mcp import config as config_module
from ix_notebook_mcp import runtime, tools
from ix_notebook_mcp.config import Config


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict[str, Any]) -> None:
    """A controlled shared namespace with the helper surface as the baseline,
    plus a clean per-session map, mirroring what install() leaves behind."""
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


def run_cell(code: str, session: str | None = None) -> runtime.Job:
    """Run one cell through the real entrypoint (no store, no kernel) and
    return the finished Job."""

    async def main() -> runtime.Job:
        return await runtime.__ix_run(code, budget=5.0, session=session)

    return asyncio.run(main())


# --------------------------------------------------------------------------- #
# per-session namespaces: isolation, persistence, the shared helper area
# --------------------------------------------------------------------------- #


def test_two_sessions_do_not_clobber_each_others_variables(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {"Result": runtime.Result})
    # The production failure: both agents bind the same name in parallel.
    run_cell("x = 'agent-a'\nResult.ok('a')", session="sess-a")
    run_cell("x = 'agent-b'\nResult.ok('b')", session="sess-b")
    a = run_cell("Result.text(x)", session="sess-a")
    b = run_cell("Result.text(x)", session="sess-b")
    assert a.status == "done"
    assert a.result.llm_result == "agent-a"
    assert b.status == "done"
    assert b.result.llm_result == "agent-b"


def test_a_session_namespace_persists_across_calls(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {"Result": runtime.Result})
    run_cell("def double(n):\n    return n * 2\nbase = 21\nResult.ok('set')", session="s1")
    job = run_cell("Result.text(str(double(base)))", session="s1")
    assert job.status == "done", (job.status, job.error)
    assert job.result.llm_result == "42"


def test_sessions_see_the_shared_helpers_but_not_each_other(monkeypatch: pytest.MonkeyPatch) -> None:
    shared = {"Result": runtime.Result, "jobs": runtime.jobs}
    _wire(monkeypatch, shared)
    # The baseline helper surface (here: Result) is visible in a fresh session...
    helper = run_cell("Result.ok('helpers reachable')", session="s1")
    assert helper.status == "done", (helper.status, helper.error)
    # ...but a name another session bound is not.
    run_cell("secret = 'mine'\nResult.ok('set')", session="s1")
    other = run_cell("Result.text('y' if 'secret' in globals() else 'n')", session="s2")
    assert other.result.llm_result == "n"


def test_session_assignments_never_leak_into_the_shared_namespace(monkeypatch: pytest.MonkeyPatch) -> None:
    shared = {"Result": runtime.Result}
    _wire(monkeypatch, shared)
    run_cell("leaky = 1\nResult.ok('set')", session="s1")
    assert "leaky" not in shared
    # And user state bound in the shared namespace AFTER the baseline stays out
    # of a new session's seed (only the helper area is shared).
    shared["late_user_var"] = 7
    job = run_cell("Result.text('y' if 'late_user_var' in globals() else 'n')", session="s2")
    assert job.result.llm_result == "n"


def test_no_session_keeps_the_single_shared_namespace(monkeypatch: pytest.MonkeyPatch) -> None:
    shared = {"Result": runtime.Result}
    _wire(monkeypatch, shared)
    run_cell("x = 40\nResult.ok('set')")
    job = run_cell("Result.text(str(x + 2))")
    assert job.result.llm_result == "42"
    assert shared["x"] == 40  # writes land in the shared dict, as before


def test_ix_read_evaluates_in_the_callers_session(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {"Result": runtime.Result})
    run_cell("answer = 41\nResult.ok('set')", session="s1")

    async def reads() -> tuple[runtime.Result, runtime.Result]:
        own = await runtime.__ix_read("answer + 1", session="s1")
        try:
            other = await runtime.__ix_read("answer + 1", session="s2")
        except NameError as exc:
            other = exc
        return own, other

    own, other = asyncio.run(reads())
    assert own.llm_result == "42"
    assert isinstance(other, NameError)  # s2 never bound `answer`


# --------------------------------------------------------------------------- #
# the server-side session key: stable per MCP session, HTTP transport only
# --------------------------------------------------------------------------- #


class _FakeCtx:
    def __init__(self, session: object) -> None:
        self.session = session


class _FakeSession:
    pass


def test_session_id_is_stable_per_session_and_distinct_across(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    monkeypatch.setattr(config_module, "_CONFIG", Config(workdir=Path(tmp_path), transport="http"))
    one, two = _FakeSession(), _FakeSession()
    first = tools._session_id(_FakeCtx(one))
    assert first is not None
    assert tools._session_id(_FakeCtx(one)) == first
    assert tools._session_id(_FakeCtx(two)) != first


def test_session_id_is_none_on_stdio_so_checkpointing_keeps_working(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # stdio serves one client per process; its state must stay in the shared
    # namespace so `serve --session FILE` checkpoint/restore still covers it.
    monkeypatch.setattr(config_module, "_CONFIG", Config(workdir=Path(tmp_path), transport="stdio"))
    assert tools._session_id(_FakeCtx(_FakeSession())) is None


# --------------------------------------------------------------------------- #
# Result ergonomics: a None / print-only cell auto-returns instead of erroring
# --------------------------------------------------------------------------- #


def _capture_stdout(monkeypatch: pytest.MonkeyPatch) -> None:
    """Route prints to the running job's buffer the way install() does (the
    in-process tests skip install, so wire the tee explicitly)."""
    monkeypatch.setattr(sys, "stdout", runtime._Tee(sys.stdout))


def test_print_only_cell_auto_returns_its_stdout(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})
    _capture_stdout(monkeypatch)
    job = run_cell("print('hello-from-stdout')")
    assert job.status == "done", (job.status, job.error)
    assert isinstance(job.result, runtime.Result)
    assert "hello-from-stdout" in job.result.llm_result


def test_assignment_only_cell_auto_oks_quietly(monkeypatch: pytest.MonkeyPatch) -> None:
    shared: dict = {}
    _wire(monkeypatch, shared)
    job = run_cell("x = 5")
    assert job.status == "done", (job.status, job.error)
    assert shared["x"] == 5
    assert "done" in job.result.llm_result  # a quiet confirmation, not stdout


def test_auto_returned_stdout_is_clipped_with_a_paging_pointer(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})
    _capture_stdout(monkeypatch)
    monkeypatch.setattr(runtime, "_AUTO_RESULT_CHARS", 100)
    job = run_cell("print('z' * 500)")
    assert job.status == "done", (job.status, job.error)
    assert f"jobs['{job.id}'].output" in job.result.llm_result
    assert len(job.result.llm_result) < 500


def test_a_bare_scalar_is_the_result_jupyter_style(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})
    job = run_cell("1 + 1")
    assert job.status == "done", (job.status, job.error)
    assert "2" in job.result.llm_result


def test_stdout_rides_with_a_bare_final_value(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})
    _capture_stdout(monkeypatch)
    job = run_cell("print('logged')\n40 + 2")
    assert job.status == "done", (job.status, job.error)
    assert "logged" in job.result.llm_result
    assert "42" in job.result.llm_result


def test_an_explicit_result_is_unchanged(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {"Result": runtime.Result})
    job = run_cell("print('noise')\nResult.text('explicit')")
    assert job.status == "done"
    assert job.result.llm_result == "explicit"


def test_repr_html_and_repr_llm_split_bare_object_output(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})
    job = run_cell(
        "class Widget:\n"
        "    def _repr_html_(self):\n"
        "        return '<strong>human</strong>'\n"
        "    def _repr_llm_(self):\n"
        "        return 'model-view'\n"
        "Widget()"
    )
    assert job.status == "done", (job.status, job.error)
    assert job.result.user_html == "<strong>human</strong>"
    assert job.result.llm_result == "model-view"


def test_polars_dataframe_defaults_to_compact_nuon_for_llm(monkeypatch: pytest.MonkeyPatch) -> None:
    pl = pytest.importorskip("polars")
    _wire(monkeypatch, {"pl": pl})
    job = run_cell("pl.DataFrame({'name': ['ada', 'grace'], 'score': [10, 11]})")
    assert job.status == "done", (job.status, job.error)
    assert "shape: (2, 2)" in job.result.llm_result
    assert "[[name, score]; [\"ada\", 10], [\"grace\", 11]]" in job.result.llm_result
    assert "┌" not in job.result.llm_result

    nu = shutil.which("nu")
    if nu is None:
        pytest.skip("nushell is required to parse the NUON e2e")
    body = job.result.llm_result.split("\n", 1)[1]
    parsed = subprocess.run(
        [nu, "-c", "open --raw /dev/stdin | from nuon | get 1.name"],
        input=body,
        text=True,
        capture_output=True,
        check=True,
    )
    assert parsed.stdout.strip() == "grace"


def test_list_of_records_uses_nushell_table_nuon() -> None:
    assert runtime._nuon([{"a": 1, "b": 2}, {"a": 5, "b": 7}]) == "[[a, b]; [1, 2], [5, 7]]"
