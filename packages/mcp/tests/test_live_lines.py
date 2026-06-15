"""The live-line and error-line surface: the kernel runtime samples the cell
line a running job is executing, records the cell line a failure was raised on,
trims tracebacks to start at the cell, and the dashboard emits line-addressable
code HTML whose data-line numbers match the compiler's (and so the traceback's).
"""

from __future__ import annotations

import asyncio

from ix_notebook_mcp import runtime, store


def run_cell(code: str, ns: dict | None = None) -> runtime.Job:
    """Run one cell through the real runner (no store, no kernel) and return the
    finished Job."""

    async def main() -> runtime.Job:
        job = runtime.Job(code, budget=5)
        job.task = asyncio.ensure_future(runtime._runner(job, ns if ns is not None else {}))
        await asyncio.wait({job.task})
        return job

    return asyncio.run(main())


# --------------------------------------------------------------------------- #
# error_line + trimmed tracebacks
# --------------------------------------------------------------------------- #


def test_error_line_is_the_failing_cell_line() -> None:
    job = run_cell("x = 1\n1 / 0\n")
    assert job.status == "error"
    assert job.error_line == 2
    # The traceback starts at the cell, not at the kernel's runner/eval plumbing.
    assert job.error.startswith("Traceback")
    assert "runtime.py" not in job.error
    assert f'File "<job {job.id}>", line 2' in job.error


def test_error_line_inside_a_helper_is_the_deepest_cell_frame() -> None:
    job = run_cell("def boom():\n    raise ValueError('x')\nboom()\n")
    assert job.status == "error"
    assert job.error_line == 2  # the raise inside boom(), not the call on line 3


def test_error_line_through_a_library_frame_points_at_the_cell() -> None:
    # The raise happens inside json (a library frame); the recorded line is the
    # deepest *cell* frame: the loads() call on line 2.
    job = run_cell("import json\njson.loads('not json')\n")
    assert job.status == "error"
    assert job.error_line == 2


def test_syntax_error_records_its_line_without_plumbing_frames() -> None:
    job = run_cell("x = 1\ndef f(:\n")
    assert job.status == "error"
    assert job.error_line == 2
    assert "SyntaxError" in job.error
    assert "_compile" not in job.error and "runtime.py" not in job.error


def test_generator_cell_keeps_real_line_numbers_on_error() -> None:
    job = run_cell("yield Result.ok('a')\n1 / 0\n", ns={"Result": runtime.Result})
    assert job.status == "error"
    assert job.error_line == 2


# --------------------------------------------------------------------------- #
# the live executing line
# --------------------------------------------------------------------------- #


def test_current_line_tracks_the_suspended_await() -> None:
    async def main() -> None:
        code = "import asyncio\nawait asyncio.sleep(30)\nResult.ok('x')\n"
        job = runtime.Job(code, budget=5)
        job.task = asyncio.ensure_future(runtime._runner(job, {"Result": runtime.Result}))
        await asyncio.sleep(0.1)
        assert runtime._current_line(job) == 2
        job.cancel()
        await asyncio.wait({job.task})
        assert job.status == "cancelled"
        # A finished job has no current line.
        assert runtime._current_line(job) is None

    asyncio.run(main())


def test_current_line_advances_between_awaits() -> None:
    async def main() -> None:
        code = (
            "import asyncio\n"
            "await asyncio.sleep(0.05)\n"
            "await asyncio.sleep(30)\n"
            "Result.ok('x')\n"
        )
        job = runtime.Job(code, budget=5)
        job.task = asyncio.ensure_future(runtime._runner(job, {"Result": runtime.Result}))
        await asyncio.sleep(0.01)
        first = runtime._current_line(job)
        await asyncio.sleep(0.2)
        second = runtime._current_line(job)
        assert (first, second) == (2, 3)
        job.cancel()
        await asyncio.wait({job.task})

    asyncio.run(main())


def test_sync_cell_has_no_current_line() -> None:
    job = run_cell("x = 1\nResult.ok('done')\n", ns={"Result": runtime.Result})
    assert job.status == "done"
    assert runtime._current_line(job) is None


# --------------------------------------------------------------------------- #
# store round-trip
# --------------------------------------------------------------------------- #


def test_store_round_trips_line_and_error_line(tmp_path) -> None:
    conn = store.connect(tmp_path / "exec.sqlite")
    store.start(conn, id="j1", name="j1", code="x", started_at=0.0)
    store.update_output(conn, "j1", "out", line=3)
    assert store.get(conn, "j1")["line"] == 3
    store.finish(
        conn,
        id="j1",
        status="error",
        ended_at=1.0,
        output="out",
        result=None,
        error="boom",
        error_line=2,
    )
    row = store.get(conn, "j1")
    assert row["error_line"] == 2
    assert row["line"] is None  # finished: no live line
