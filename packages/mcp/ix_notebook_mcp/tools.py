"""The MCP tool surface: notebook-cell operations on the live notebook.

Tools are thin: they resolve the single running :class:`~ix_notebook_mcp.app.NotebookApp`
and the active notebook, then delegate to the pure cell transforms in
:mod:`ix_notebook_mcp.cells` and the app's kernel methods. Every notebook edit
goes through the live ``YNotebook``, so a human co-editing in JupyterLab sees each
change as it happens. Schemas are derived from the type hints by FastMCP.
"""

from __future__ import annotations

import asyncio
import json
import os
from typing import Annotated, Any

from mcp.server.fastmcp import FastMCP
from pydantic import Field

from . import cells, outputs
from .app import NotebookApp, current_app

mcp = FastMCP(
    "ix-mcp",
    instructions=(
        "Drive a live Jupyter notebook. A human may have the same notebook open in "
        "JupyterLab and will see your cells and outputs appear in real time, so "
        "write the notebook as a readable narrative: markdown for context, code "
        "cells for steps. Call `notebook_use` first to pick or create a notebook. "
        "The kernel is shared with the human, and bundled modules (`tui`, `search`, "
        "`fff`, `exa_py`, `google_auth` (Gmail/Calendar via `google_auth.gmail()` / "
        "`.calendar()`), numpy, polars, duckdb, httpx, playwright, ...) import with "
        "no install step. "
        "`cell_add(run=True)` is the usual way to add and execute a step in one call."
    ),
)

Content = list[outputs.Content]

# The notebook most recently opened with `notebook_use`; the default target for
# cell operations that omit `path`. Tool-layer convenience state, scoped to this
# one MCP client, so a module-level holder is the right granularity.
_active: str | None = None


def _target(app: NotebookApp, path: str | None) -> str:
    global _active
    if path is not None:
        return app.ensure_file(path)
    if _active is None:
        raise ValueError("no active notebook; call notebook_use(path) first")
    return _active


@mcp.tool(
    description="Open or create a notebook and make it the active target for cell "
    "operations. Returns the path and the JupyterLab URL a human can open to "
    "co-edit it live."
)
async def notebook_use(
    path: Annotated[str, Field(description="Notebook path relative to the workspace, e.g. analysis.ipynb")],
) -> str:
    global _active
    app = current_app()
    rel = app.ensure_file(path)
    await app.live_notebook(rel)  # open the room now so a browser attaches to it
    await app.kernel_id(rel)
    _active = rel
    return json.dumps({"path": rel, "lab_url": app.lab_url(), "active": True})


@mcp.tool(description="List the notebooks in the workspace.")
async def notebook_list() -> str:
    app = current_app()
    workdir = app.config.workdir
    found = sorted(p.relative_to(workdir).as_posix() for p in workdir.rglob("*.ipynb"))
    return json.dumps({"workspace": str(workdir), "notebooks": found, "active": _active})


@mcp.tool(description="Read the cells of the active (or given) notebook: index, type, source, and output summary.")
async def notebook_read(
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> str:
    app = current_app()
    rel = _target(app, path)
    ynb = await app.live_notebook(rel)
    listing = [
        {
            "index": cell["index"],
            "id": cell.get("id"),
            "cell_type": cell.get("cell_type"),
            "execution_count": cell.get("execution_count"),
            "source": cell.get("source", ""),
            "output_count": len(cell.get("outputs", [])),
        }
        for cell in cells.read_all(ynb)
    ]
    return json.dumps({"path": rel, "cells": listing})


@mcp.tool(
    description="Add a cell to the active (or given) notebook. With run=True (code "
    "cells only) it also executes the cell on the shared kernel and returns the "
    "outputs. index=-1 appends."
)
async def cell_add(
    source: Annotated[str, Field(description="Cell source")],
    cell_type: Annotated[str, Field(description="code | markdown | raw")] = "code",
    index: Annotated[int, Field(description="Insertion index; -1 appends")] = -1,
    run: Annotated[bool, Field(description="Execute after inserting (code cells only)")] = False,
    timeout: Annotated[float, Field(description="Execution timeout in seconds")] = 120.0,
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> Content:
    app = current_app()
    rel = _target(app, path)
    ynb = await app.live_notebook(rel)
    placed = cells.add(ynb, source, cell_type, index)
    header = outputs.text(json.dumps({"added": {"index": placed["index"], "id": placed.get("id"), "cell_type": cell_type}}))
    if run and cell_type == "code":
        return [header, *await _run(app, rel, ynb, placed.get("id"), timeout)]
    return [header]


@mcp.tool(
    description="Execute a code cell on the shared kernel; outputs are written into "
    "the live notebook and returned. Prefer `cell_id` (race-proof; cell_add returns "
    "one) over `index`, which is resolved against the notebook as it is right now."
)
async def cell_run(
    index: Annotated[int | None, Field(description="Cell index to execute (resolved at call time)")] = None,
    cell_id: Annotated[str | None, Field(description="Stable cell id (preferred; survives concurrent edits)")] = None,
    timeout: Annotated[float, Field(description="Execution timeout in seconds")] = 120.0,
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> Content:
    app = current_app()
    rel = _target(app, path)
    ynb = await app.live_notebook(rel)
    if cell_id is None:
        if index is None:
            raise ValueError("pass either cell_id (preferred) or index")
        cell_id = ynb.get_cell(index).get("id")
    return await _run(app, rel, ynb, cell_id, timeout)


@mcp.tool(description="Replace a cell's source (clears its outputs). With run=True, re-executes a code cell afterward.")
async def cell_overwrite(
    index: Annotated[int, Field(description="Cell index to overwrite")],
    source: Annotated[str, Field(description="New cell source")],
    run: Annotated[bool, Field(description="Re-execute after overwriting (code cells only)")] = False,
    timeout: Annotated[float, Field(description="Execution timeout in seconds")] = 120.0,
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> Content:
    app = current_app()
    rel = _target(app, path)
    ynb = await app.live_notebook(rel)
    cell = cells.overwrite_source(ynb, index, source)
    header = outputs.text(json.dumps({"overwrote": index}))
    if run and cell.get("cell_type") == "code":
        return [header, *await _run(app, rel, ynb, cell.get("id"), timeout)]
    return [header]


@mcp.tool(description="Delete a cell by index from the active (or given) notebook.")
async def cell_delete(
    index: Annotated[int, Field(description="Cell index to delete")],
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> str:
    app = current_app()
    rel = _target(app, path)
    ynb = await app.live_notebook(rel)
    cells.delete(ynb, index)
    return json.dumps({"deleted": index, "path": rel})


@mcp.tool(
    description="Run code on the shared kernel WITHOUT adding a cell to the notebook "
    "(scratch evaluation, magics, shell). Returns outputs but leaves the notebook "
    "unchanged."
)
async def run_code(
    code: Annotated[str, Field(description="Code to execute")],
    timeout: Annotated[float, Field(description="Execution timeout in seconds")] = 120.0,
    path: Annotated[str | None, Field(description="Notebook whose kernel to use; defaults to the active notebook")] = None,
) -> Content:
    app = current_app()
    rel = _target(app, path)
    cell_outputs, _ = await app.execute(rel, code, timeout)
    return outputs.to_mcp(cell_outputs)


@mcp.tool(description="Restart the shared kernel for the active (or given) notebook (clears all in-memory state).")
async def kernel_restart(
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> str:
    app = current_app()
    rel = _target(app, path)
    await app.restart_kernel(rel)
    return json.dumps({"restarted": rel})


@mcp.tool(
    description="Read-only semantic search over the shared `index` corpus (code plus "
    "Claude/Codex/shell history across the fleet). Scope with source, user, repo, "
    "host, project. Returns matching chunks as JSON."
)
async def search_semantic(
    query: str,
    top_k: int = 10,
    source: list[str] | None = None,
    user: list[str] | None = None,
    repo: str | None = None,
    host: list[str] | None = None,
    project: list[str] | None = None,
) -> str:
    import search as _search

    hits = await _search.semantic(query, top_k=top_k, **_scope(source, user, repo, host, project))
    return json.dumps(hits)


@mcp.tool(description="Read-only regex grep over the same shared `index` corpus the semantic search covers.")
async def search_grep(
    pattern: str,
    top_k: int = 10,
    case_sensitive: bool = False,
    source: list[str] | None = None,
    user: list[str] | None = None,
    repo: str | None = None,
    host: list[str] | None = None,
    project: list[str] | None = None,
) -> str:
    import search as _search

    hits = await _search.grep(pattern, top_k=top_k, case_sensitive=case_sensitive, **_scope(source, user, repo, host, project))
    return json.dumps(hits)


def _scope(
    source: list[str] | None,
    user: list[str] | None,
    repo: str | None,
    host: list[str] | None,
    project: list[str] | None,
) -> dict[str, Any]:
    return {
        key: value
        for key, value in (("source", source), ("user", user), ("repo", repo), ("host", host), ("project", project))
        if value
    }


@mcp.tool(
    description="List Google Calendar events in a window (default: now through 7 days "
    "from now). Times are RFC 3339, `YYYY-MM-DD HH:MM` (host-local), or `YYYY-MM-DD`. "
    "Returns the events as a JSON array. Needs a one-time `gcal auth` on this host."
)
async def calendar_events(
    time_min: Annotated[str | None, Field(description="Window start; default now")] = None,
    time_max: Annotated[str | None, Field(description="Window end; default a week after the start")] = None,
    query: Annotated[str | None, Field(description="Free-text filter over summary, description, attendees")] = None,
    max_events: Annotated[int, Field(description="Maximum number of events")] = 20,
    calendar: Annotated[str, Field(description="Calendar id: an email, or `primary`")] = "primary",
) -> str:
    args = ["list", "--json", "--calendar", calendar, "--max", str(max_events)]
    if time_min:
        args += ["--from", time_min]
    if time_max:
        args += ["--to", time_max]
    if query:
        args += ["--query", query]
    return await _gcal(*args)


@mcp.tool(
    description="Create a Google Calendar event and return it as JSON. Timed events "
    "take RFC 3339 or host-local `YYYY-MM-DD HH:MM` for start/end; with all_day=True "
    "they are dates and end is the last day, inclusive. By default Google emails every "
    "attendee (notify='all'); pass notify='none' to stay silent."
)
async def calendar_event_create(
    summary: Annotated[str, Field(description="Event title")],
    start: Annotated[str, Field(description="Start time, or a date with all_day")],
    end: Annotated[str | None, Field(description="End time; required unless all_day")] = None,
    all_day: Annotated[bool, Field(description="All-day event (start/end are dates)")] = False,
    description: Annotated[str | None, Field(description="Free-text body")] = None,
    location: Annotated[str | None, Field(description="Free-text location")] = None,
    attendees: Annotated[list[str] | None, Field(description="Attendee emails to invite")] = None,
    notify: Annotated[str, Field(description="Who Google emails: all | external-only | none")] = "all",
    calendar: Annotated[str, Field(description="Calendar id: an email, or `primary`")] = "primary",
) -> str:
    args = [
        "create", "--json", "--calendar", calendar,
        "--summary", summary, "--start", start, "--notify", notify,
    ]
    if end:
        args += ["--end", end]
    if all_day:
        args.append("--all-day")
    if description:
        args += ["--description", description]
    if location:
        args += ["--location", location]
    for attendee in attendees or []:
        args += ["--attendee", attendee]
    return await _gcal(*args)


@mcp.tool(
    description="Cancel (delete) a Google Calendar event by id. By default Google "
    "emails every attendee about the cancellation; pass notify='none' to stay silent."
)
async def calendar_event_cancel(
    event_id: Annotated[str, Field(description="The event id, as returned by calendar_events")],
    notify: Annotated[str, Field(description="Who Google emails: all | external-only | none")] = "all",
    calendar: Annotated[str, Field(description="Calendar id: an email, or `primary`")] = "primary",
) -> str:
    return await _gcal(
        "cancel", event_id, "--json", "--calendar", calendar, "--notify", notify
    )


async def _gcal(*args: str) -> str:
    """Run the bundled ``gcal`` binary and return its stdout.

    The calendar tools stay a thin binding per RFC 0003: the `google-calendar`
    Rust crate owns the API client, OAuth, and error mapping; ``gcal --json``
    is the machine contract this layer forwards verbatim. The wrapper sets
    IX_GCAL_BIN (same shape as IX_VMKIT_BIN for the vmkit helper).
    """
    binary = os.environ.get("IX_GCAL_BIN")
    if not binary:
        raise RuntimeError("IX_GCAL_BIN is not set; the gcal binary is bundled into ix-mcp")
    proc = await asyncio.create_subprocess_exec(
        binary,
        *args,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    stdout, stderr = await proc.communicate()
    if proc.returncode != 0:
        detail = stderr.decode(errors="replace").strip()
        raise RuntimeError(detail or f"gcal exited with status {proc.returncode}")
    return stdout.decode(errors="replace")


async def _run(app: NotebookApp, rel: str, ynb: Any, cell_id: str | None, timeout: float) -> Content:
    """Execute the cell with ``cell_id``, write its outputs back, and return them.

    Re-resolves the index by id right before reading the source and again before
    writing outputs, so a concurrent insert/delete by the human cannot make us run
    or overwrite the wrong cell.
    """
    if cell_id is None:
        raise ValueError("cell has no id")
    source = ynb.get_cell(cells.index_of(ynb, cell_id)).get("source", "")
    try:
        cell_outputs, execution_count = await app.execute(rel, source, timeout)
    except TimeoutError:
        cells.set_outputs(ynb, cells.index_of(ynb, cell_id), [outputs.error_output("TimeoutError", f"cell exceeded {timeout}s")], None)
        return [outputs.text(f"cell timed out after {timeout}s")]
    cells.set_outputs(ynb, cells.index_of(ynb, cell_id), cell_outputs, execution_count)
    return outputs.to_mcp(cell_outputs)
