"""A cell that rebinds or deletes a kernel builtin gets it restored (issue #2430).

``api = await nu(...)`` used to silently destroy the ``api`` catalog helper for
the rest of the session, and ``del api`` afterwards raised NameError instead of
bringing the builtin back (the injected binding was gone, not shadowed), so the
failure surfaced cells later and looked like a broken kernel. The runner now
re-injects every install()-bound builtin after each cell and drops a one-line
warning naming the clobbered name into the job's output.
"""

from __future__ import annotations

import asyncio

import pytest

from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict, protected: dict) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})
    monkeypatch.setattr(runtime, "_protected_builtins", protected)


def _run(code: str) -> runtime.Job:
    return asyncio.run(runtime.__ix_run(code, budget=5.0))


def test_rebinding_a_builtin_restores_it_and_warns(monkeypatch: pytest.MonkeyPatch) -> None:
    ns = {"api": runtime.api}
    _wire(monkeypatch, ns, {"api": runtime.api})
    job = _run("api = 'clobbered'")
    assert job.status == "done"
    assert ns["api"] is runtime.api, "the builtin must be re-injected after the cell"
    assert "'api' is a kernel builtin" in job.output


def test_deleting_a_builtin_restores_it(monkeypatch: pytest.MonkeyPatch) -> None:
    ns = {"api": runtime.api}
    _wire(monkeypatch, ns, {"api": runtime.api})
    job = _run("del api")
    assert job.status == "done"
    assert ns["api"] is runtime.api
    assert "'api' is a kernel builtin" in job.output


def test_error_cells_restore_too(monkeypatch: pytest.MonkeyPatch) -> None:
    # The restore runs in the runner's finally, so a cell that clobbers a builtin
    # and then blows up still leaves the surface intact for the next cell.
    ns = {"api": runtime.api}
    _wire(monkeypatch, ns, {"api": runtime.api})
    job = _run("api = 42\nraise ValueError('boom')")
    assert job.status == "error"
    assert ns["api"] is runtime.api
    assert "'api' is a kernel builtin" in job.output


def test_untouched_builtins_stay_silent(monkeypatch: pytest.MonkeyPatch) -> None:
    ns = {"api": runtime.api}
    _wire(monkeypatch, ns, {"api": runtime.api})
    job = _run("x = 1")
    assert job.status == "done"
    assert "kernel builtin" not in job.output
    assert ns["x"] == 1, "ordinary user bindings are untouched"


def test_reimporting_the_same_module_is_not_a_clobber(monkeypatch: pytest.MonkeyPatch) -> None:
    # `import json` rebinds the name to the same module object install() bound;
    # identity comparison keeps that silent.
    import json as json_mod

    ns = {"json": json_mod}
    _wire(monkeypatch, ns, {"json": json_mod})
    job = _run("import json")
    assert job.status == "done"
    assert "kernel builtin" not in job.output


def test_install_registers_builtins_but_not_lazy_modules(monkeypatch: pytest.MonkeyPatch) -> None:
    # End-to-end against the real install(): the helper surface (api, jobs, and
    # the preimported modules) is protected; lazy-proxied module names are NOT --
    # a user variable shadowing one (a temp `x`, say) deliberately stays user
    # state (see the lazy-binding comment in install()).
    ns: dict = {}
    runtime.install(ns)
    assert runtime._protected_builtins["api"] is runtime.api
    assert "jobs" in runtime._protected_builtins
    for lazy_name in runtime._lazy_module_names:
        assert lazy_name not in runtime._protected_builtins
