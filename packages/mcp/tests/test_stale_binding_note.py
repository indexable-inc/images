"""A failed cell's traceback names the bindings it never updated (issue #2526).

A cell that raises partway leaves every assignment at/after the failing line
unexecuted while the namespace keeps each name's old value, so a retry cell
that reads those names silently operates on stale state (the incident: a retry
rebuilt an email from a ``body`` the failed cell never reassigned and re-sent
the previous one). The runner now appends ``NOTE: this cell failed before
updating: ...`` to the traceback; ``introspect.unreached_bindings`` supplies
the names and makes only provable claims.
"""

from __future__ import annotations

import asyncio

import pytest

from ix_notebook_mcp import runtime
from ix_notebook_mcp.introspect import unreached_bindings


class TestUnreachedBindings:
    """The static walk: which bindings provably never ran."""

    def test_linear_cell_names_the_assignments_at_and_after_the_failing_line(self) -> None:
        # The incident shape: line 2 raises, so `mid` (the failing statement's
        # own target: its right side raised first), `body`, and `msg` never
        # (re)bound.
        code = (
            "meta = fetch()\n"
            "mid = meta['Message-ID']\n"
            "body = 'Following up'\n"
            "msg = build(body)\n"
        )
        assert unreached_bindings(code, 2) == ["mid", "body", "msg"]

    def test_bindings_before_the_failing_line_are_not_claimed(self) -> None:
        assert unreached_bindings("meta = 1\nboom()\nx = 2\n", 2) == ["x"]

    def test_multiline_statement_containing_the_failure_is_claimed(self) -> None:
        # The statement starts before the failing line but binds only at
        # completion, which it never reached.
        code = "x = build(\n    boom(),\n)\ny = 1\n"
        assert unreached_bindings(code, 2) == ["x", "y"]

    def test_enclosing_loop_body_is_not_claimed(self) -> None:
        # An earlier iteration may have bound `x`, `y`, and the loop target;
        # only `z`, outside the loop, is provable.
        code = "for i in gen():\n    x = step(i)\n    boom()\n    y = 2\nz = 3\n"
        assert unreached_bindings(code, 3) == ["z"]

    def test_loops_entirely_after_the_failing_line_are_claimed(self) -> None:
        code = "boom()\nfor i in gen():\n    x = step(i)\n"
        assert unreached_bindings(code, 1) == ["i", "x"]

    def test_handlers_and_finally_of_the_enclosing_try_are_not_claimed(self) -> None:
        # The unwind runs `finally` (and a handler may have run and re-raised),
        # so only the try body's `a` and the post-try `d` are provable.
        code = (
            "try:\n"
            "    boom()\n"
            "    a = 1\n"
            "except ValueError:\n"
            "    b = 2\n"
            "finally:\n"
            "    c = 3\n"
            "d = 4\n"
        )
        assert unreached_bindings(code, 2) == ["a", "d"]

    def test_nested_function_locals_are_not_claimed_but_the_def_name_is(self) -> None:
        code = "boom()\ndef helper():\n    local = 1\n"
        assert unreached_bindings(code, 1) == ["helper"]

    def test_import_and_unpack_targets_count(self) -> None:
        code = "boom()\nimport json as j\na, (b, *rest) = f()\n"
        assert unreached_bindings(code, 1) == ["j", "a", "b", "rest"]

    def test_walrus_after_the_failing_line_counts_same_line_does_not(self) -> None:
        # `n` may have bound before `boom()` raised on the same line (order
        # within a line is unknowable), so only `total` (statement completion)
        # and `m` (strictly after) are claimed.
        code = "total = (n := count()) + boom()\nif (m := match()):\n    pass\n"
        assert unreached_bindings(code, 1) == ["total", "m"]

    def test_attribute_and_subscript_targets_are_not_names(self) -> None:
        assert unreached_bindings("boom()\nobj.attr = 1\nd['k'] = 2\n", 1) == []

    def test_unparseable_code_yields_nothing(self) -> None:
        assert unreached_bindings("def (", 1) == []


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


def _run(code: str) -> runtime.Job:
    return asyncio.run(runtime.__ix_run(code, budget=5.0))


def _note(job: runtime.Job) -> str:
    """The NOTE line of a failed job's error, or ''."""
    for line in (job.error or "").splitlines():
        if line.startswith("NOTE:"):
            return line
    return ""


class TestStaleBindingNote:
    """End to end: the note rides the failed cell's recorded traceback."""

    def test_failed_cell_traceback_names_unreached_assignments(self, monkeypatch: pytest.MonkeyPatch) -> None:
        _wire(monkeypatch, {})
        job = _run("meta = {}\nmid = meta['Message-ID']\nbody = 'x'\nmsg = body\n")
        assert job.status == "error"
        assert "KeyError" in (job.error or "")
        assert "NOTE: this cell failed before updating: mid, body, msg" in (job.error or "")

    def test_failure_on_the_last_line_has_no_note(self, monkeypatch: pytest.MonkeyPatch) -> None:
        _wire(monkeypatch, {})
        job = _run("x = 1\nraise ValueError('boom')\n")
        assert job.status == "error"
        assert _note(job) == ""

    def test_syntax_error_has_no_note(self, monkeypatch: pytest.MonkeyPatch) -> None:
        # Nothing ran at all; the SyntaxError already says so.
        _wire(monkeypatch, {})
        job = _run("x = 1\ndef (\n")
        assert job.status == "error"
        assert _note(job) == ""

    def test_raise_inside_a_cell_defined_helper_uses_the_toplevel_line(self, monkeypatch: pytest.MonkeyPatch) -> None:
        # The deepest cell frame is line 2 (inside helper); the note must key
        # off the top-level call at line 4, so `x` (already bound) is not
        # claimed and `y` is.
        _wire(monkeypatch, {})
        code = (
            "def helper():\n"
            "    raise ValueError('boom')\n"
            "x = 1\n"
            "helper()\n"
            "y = 2\n"
        )
        job = _run(code)
        assert job.status == "error"
        assert _note(job) == (
            "NOTE: this cell failed before updating: y -- a retry "
            "that reads these names gets values from before this run."
        )

    def test_yielding_cell_gets_the_note_too(self, monkeypatch: pytest.MonkeyPatch) -> None:
        # Gen-mode cells run inside __ix_cell__, whose frame keeps the
        # original line numbers.
        _wire(monkeypatch, {})
        job = _run("yield 1\nint('nope')\nz = 5\n")
        assert job.status == "error"
        assert "NOTE: this cell failed before updating: z" in (job.error or "")

    def test_user_keyboard_interrupt_gets_the_note(self, monkeypatch: pytest.MonkeyPatch) -> None:
        # An interrupt stops the cell mid-run exactly like an exception.
        _wire(monkeypatch, {})
        job = _run("raise KeyboardInterrupt()\nx = 1\n")
        assert job.status == "error"
        assert "NOTE: this cell failed before updating: x" in (job.error or "")

    def test_successful_cell_has_no_note(self, monkeypatch: pytest.MonkeyPatch) -> None:
        _wire(monkeypatch, {})
        job = _run("x = 1\ny = 2\n")
        assert job.status == "done"
        assert job.error is None

    def test_many_names_are_elided(self, monkeypatch: pytest.MonkeyPatch) -> None:
        _wire(monkeypatch, {})
        lines = "int('nope')\n" + "".join(f"v{i} = {i}\n" for i in range(15))
        job = _run(lines)
        assert job.status == "error"
        note = _note(job)
        assert "v11" in note
        assert "v12" not in note
        assert "+3 more" in note
