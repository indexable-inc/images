"""A failed cell names the bindings it never reached (issue #2526).

A cell that raises partway through leaves every assignment at/after the failing
statement unexecuted while the namespace keeps the old values; a retry cell that
reuses those names silently operates on stale state (the incident: a rebuilt
email reused the previous run's ``body`` and sent a vendor rep a duplicate).
The runner now appends ``NOTE: this cell failed before updating: ...`` to the
traceback, computed statically by :func:`introspect.unexecuted_bindings` from
the top-level line execution stopped on.
"""

from __future__ import annotations

import asyncio

import pytest

from ix_notebook_mcp import runtime
from ix_notebook_mcp.introspect import unexecuted_bindings


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict) -> None:
    # The note is an execution-time diagnostic; disable the per-cell type check
    # so deliberately type-wrong cells (the TypeError-hint case) still run.
    monkeypatch.setenv("IX_MCP_TYPECHECK", "0")
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


def _run(code: str) -> runtime.Job:
    return asyncio.run(runtime.__ix_run(code, budget=5.0))


class TestUnexecutedBindings:
    """The static walk: which names a run stopped on a line never (re)bound."""

    def test_names_at_and_after_the_stop_line(self) -> None:
        code = "existing = 'old'\nraise KeyError('Message-ID')\nbody = 'x'\nmsg = body\n"
        assert unexecuted_bindings(code, 2) == ["body", "msg"]

    def test_multiline_assignment_straddling_the_failure_reports_its_target(self) -> None:
        # The target's own lineno is BEFORE the failing expression line; the
        # statement extent is what decides.
        code = "x = (\n    1 / 0\n)\ny = 2\n"
        assert unexecuted_bindings(code, 1) == ["x", "y"]

    def test_def_bodies_are_locals_not_cell_bindings(self) -> None:
        code = "def f():\n    inner = 1\n    raise ValueError\nx = 1\ny = f()\nz = 2\n"
        # Stop line 5 is the call site; f and x already bound, inner is a local.
        assert unexecuted_bindings(code, 5) == ["y", "z"]

    def test_loop_target_bound_when_the_body_fails(self) -> None:
        # A failure inside the body means the header bound this iteration.
        assert unexecuted_bindings("for i in range(3):\n    acc = 1 / 0\ndone = True\n", 2) == [
            "acc",
            "done",
        ]
        # Stopping on the header itself means the target never (re)bound.
        assert unexecuted_bindings("for i in bad():\n    acc = i\ndone = True\n", 1) == [
            "i",
            "acc",
            "done",
        ]

    def test_comprehension_targets_do_not_leak_but_walrus_does(self) -> None:
        code = "raise ValueError\nrows = [r for r in data]\ntotal = (t := sum(rows))\n"
        assert unexecuted_bindings(code, 1) == ["rows", "total", "t"]

    def test_import_def_and_class_names_count(self) -> None:
        code = "raise ValueError\nimport json as j\ndef g(): pass\nclass C: pass\n"
        assert unexecuted_bindings(code, 1) == ["j", "g", "C"]

    def test_unparseable_code_yields_nothing(self) -> None:
        assert unexecuted_bindings("def broken(:\n", 1) == []


class TestFailureNote:
    """The runner appends the note to the traceback of a failed cell."""

    def test_incident_shape_names_the_stale_bindings(self, monkeypatch: pytest.MonkeyPatch) -> None:
        # The 2026-07-08 incident: a raise before `body = ...` left the previous
        # run's body in the namespace and the retry sent a duplicate email.
        _wire(monkeypatch, {})
        job = _run(
            "existing = 'old'\n"
            "raise KeyError('Message-ID')\n"
            "body = 'Following up'\n"
            "msg = body\n"
        )
        assert job.status == "error"
        assert job.error is not None
        assert "NOTE: this cell failed before updating: body, msg" in job.error

    def test_failure_inside_a_cell_defined_function_stops_at_the_call_site(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        # The deepest cell frame is inside f's body; the note must key off the
        # top-level call site, not list f's locals or already-run assignments.
        _wire(monkeypatch, {})
        job = _run(
            "def f():\n"
            "    inner = 1\n"
            "    raise ValueError('boom')\n"
            "x = 1\n"
            "y = f()\n"
            "z = 2\n"
        )
        assert job.status == "error"
        assert job.error is not None
        assert "NOTE: this cell failed before updating: y, z" in job.error
        assert "inner" not in job.error.split("NOTE:")[1]

    def test_no_note_when_nothing_after_the_failure_binds(self, monkeypatch: pytest.MonkeyPatch) -> None:
        _wire(monkeypatch, {})
        job = _run("x = 1\nraise ValueError('boom')\n")
        assert job.status == "error"
        assert job.error is not None
        assert "NOTE:" not in job.error

    def test_note_rides_after_the_typeerror_hint(self, monkeypatch: pytest.MonkeyPatch) -> None:
        # Both diagnostics apply: the signature hint, then the stale-binding note.
        def kw_target(*, query: str) -> None:
            pass

        _wire(monkeypatch, {"kw_target": kw_target})
        job = _run("out = kw_target(bad_kwarg=1)\nsent = out\n")
        assert job.status == "error"
        assert job.error is not None
        assert "NOTE: this cell failed before updating: out, sent" in job.error
        if "Hint:" in job.error:
            assert job.error.index("Hint:") < job.error.index("NOTE:")

    def test_generator_cell_gets_the_note_too(self, monkeypatch: pytest.MonkeyPatch) -> None:
        # A yielding cell runs as __ix_cell__ with original line numbers; the
        # note must work through that frame as well.
        _wire(monkeypatch, {})
        job = _run("yield 1\nraise ValueError('boom')\nlater = 2\n")
        assert job.status == "error"
        assert job.error is not None
        assert "NOTE: this cell failed before updating: later" in job.error
