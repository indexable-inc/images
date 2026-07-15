"""Issue #3131: a background job wrapping ``nu(...)`` pages as real lines.

``jobs.spawn(nu('...', check=False), name=...)`` used to have no readable
output surface: ``.output`` is empty by design (the text is the job's VALUE,
not its stdout), and the pageable fallback -- the result's model text -- was
the generic tuple rendering, a mixed one-column NUON frame with every newline
escaped (``[[value]; ["..."], ["0"]]``). ``NuResult.__ix_llm__`` now renders
the command's own output (plus a trailing ``[exit N]`` marker on failure), so
``.tail()`` / ``.grep()`` / ``.lines()`` / ``.text`` page the stdout lines.
Drives the real embedded engine (the ``nu._nu`` PyO3 cdylib), like test_nu.py.
"""

from __future__ import annotations

import asyncio
import sys

import pytest

import nu
from ix_notebook_mcp import runtime


def _wire(monkeypatch: pytest.MonkeyPatch, ns: dict) -> None:
    monkeypatch.setattr(runtime, "_user_ns", ns)
    monkeypatch.setattr(runtime, "_baseline_names", frozenset(ns))
    monkeypatch.setattr(runtime, "_session_namespaces", {})


# A failing multi-line external: the issue's `bash -c "nix build ... | tail"`
# shape, using the interpreter binary the sandbox certainly has.
_SCRIPT = "print('alpha'); print('beta'); raise SystemExit(3)"


def test_spawned_nu_job_pages_stdout_lines(monkeypatch: pytest.MonkeyPatch) -> None:
    _wire(monkeypatch, {})

    async def drive() -> runtime.Job:
        job = runtime.jobs.spawn(
            nu(f'^{sys.executable} -c "{_SCRIPT}"', check=False), name="nu-build"
        )
        await job
        return job

    try:
        job = asyncio.run(drive())
    finally:
        nu.reset()
    assert job.status == "done", job.error

    # The paging helpers operate on the command's real stdout lines.
    tail = job.tail()
    assert "alpha\nbeta" in tail, tail
    assert "\\n" not in tail, tail  # no escaped newlines (the issue's symptom)
    assert "[exit 3]" in tail
    assert "0: alpha" in job.grep("alpha")
    assert "1: beta" in job.lines(1, 2)

    # The rendered result text is the same unescaped view, and the original
    # NuResult stays reachable for structured reads.
    assert job.text == "alpha\nbeta\n[exit 3]"
    result = job.result
    assert result is not None
    assert result.value.exit_code == 3
    assert result.value.result == "alpha\nbeta"
