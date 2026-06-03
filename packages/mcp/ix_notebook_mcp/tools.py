"""The MCP tool surface: notebook-cell operations on the live notebook.

Tools are defined once on a :class:`FastMCP` instance (schemas come from the type
hints) and exposed over either stdio or streamable HTTP. The notebook-mutating
tools all edit the live ``YNotebook`` so a human co-editing in JupyterLab sees
every change as it happens.

The default target for cell operations is the notebook most recently passed to
``notebook_use``; pass ``path`` explicitly to act on another notebook.
"""

from __future__ import annotations

import json
from typing import Annotated, Any

from mcp import types as mcp_types
from mcp.server.fastmcp import FastMCP
from pydantic import Field

from . import kernel, notebook
from .runtime import RUNTIME

mcp = FastMCP(
    "ix-mcp",
    instructions=(
        "Drive a live Jupyter notebook. A human may have the same notebook open "
        "in JupyterLab and will see your cells and outputs appear in real time, "
        "so write the notebook as a readable narrative: markdown for context, "
        "code cells for steps. Call `notebook_use` first to pick or create a "
        "notebook. The kernel is shared with the human, and bundled modules "
        "(`tui`, `search`, numpy, polars, duckdb, httpx, playwright, ...) import "
        "with no install step. `cell_add(run=True)` is the usual way to add and "
        "execute a step in one call."
    ),
)

ContentList = list[mcp_types.TextContent | mcp_types.ImageContent]


def _target(path: str | None) -> str:
    if path is not None:
        return notebook.ensure_notebook_file(path)
    if RUNTIME.active_notebook is None:
        raise ValueError("no active notebook; call notebook_use(path) first")
    return RUNTIME.active_notebook


@mcp.tool(
    description="Open or create a notebook and make it the active target for cell "
    "operations. Returns the path and the JupyterLab URL a human can open to "
    "co-edit it live."
)
async def notebook_use(
    path: Annotated[str, Field(description="Notebook path relative to the workspace, e.g. analysis.ipynb")],
) -> str:
    rel = notebook.ensure_notebook_file(path)
    await notebook.get_ynotebook(rel)  # open the room now so a browser attaches to it
    await kernel.ensure_kernel(rel)
    RUNTIME.active_notebook = rel
    return json.dumps({"path": rel, "lab_url": RUNTIME.lab_url(), "active": True})


@mcp.tool(description="List the notebooks in the workspace.")
async def notebook_list() -> str:
    workdir = RUNTIME.workdir
    notebooks = sorted(str(p.relative_to(workdir)) for p in workdir.rglob("*.ipynb"))
    return json.dumps({"workspace": str(workdir), "notebooks": notebooks, "active": RUNTIME.active_notebook})


@mcp.tool(description="Read the cells of the active (or given) notebook: index, type, source, and output summary.")
async def notebook_read(
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> str:
    rel = _target(path)
    ynb = await notebook.get_ynotebook(rel)
    cells = []
    for cell in notebook.read_cells(ynb):
        cells.append(
            {
                "index": cell["_index"],
                "id": cell.get("id"),
                "cell_type": cell.get("cell_type"),
                "execution_count": cell.get("execution_count"),
                "source": cell.get("source", ""),
                "output_count": len(cell.get("outputs", [])),
            }
        )
    return json.dumps({"path": rel, "cells": cells})


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
) -> ContentList:
    rel = _target(path)
    ynb = await notebook.get_ynotebook(rel)
    placed = notebook.add_cell(ynb, source, cell_type, index)
    cell_id = placed.get("id")
    header = mcp_types.TextContent(
        type="text", text=json.dumps({"added": {"index": placed["_index"], "id": cell_id, "cell_type": cell_type}})
    )
    if run and cell_type == "code":
        outputs = await _run_cell_by_id(rel, ynb, cell_id, timeout)
        return [header, *outputs]
    return [header]


@mcp.tool(description="Execute a code cell by index on the shared kernel; outputs are written into the live notebook and returned.")
async def cell_run(
    index: Annotated[int, Field(description="Cell index to execute")],
    timeout: Annotated[float, Field(description="Execution timeout in seconds")] = 120.0,
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> ContentList:
    rel = _target(path)
    ynb = await notebook.get_ynotebook(rel)
    cell_id = ynb.get_cell(index).get("id")
    return await _run_cell_by_id(rel, ynb, cell_id, timeout)


@mcp.tool(description="Replace a cell's source (clears its outputs). With run=True, re-executes a code cell afterward.")
async def cell_overwrite(
    index: Annotated[int, Field(description="Cell index to overwrite")],
    source: Annotated[str, Field(description="New cell source")],
    run: Annotated[bool, Field(description="Re-execute after overwriting (code cells only)")] = False,
    timeout: Annotated[float, Field(description="Execution timeout in seconds")] = 120.0,
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> ContentList:
    rel = _target(path)
    ynb = await notebook.get_ynotebook(rel)
    cell = notebook.overwrite_cell_source(ynb, index, source)
    header = mcp_types.TextContent(type="text", text=json.dumps({"overwrote": index}))
    if run and cell.get("cell_type") == "code":
        outputs = await _run_cell_by_id(rel, ynb, cell.get("id"), timeout)
        return [header, *outputs]
    return [header]


@mcp.tool(description="Delete a cell by index from the active (or given) notebook.")
async def cell_delete(
    index: Annotated[int, Field(description="Cell index to delete")],
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> str:
    rel = _target(path)
    ynb = await notebook.get_ynotebook(rel)
    notebook.delete_cell(ynb, index)
    return json.dumps({"deleted": index, "path": rel})


@mcp.tool(
    description="Run code on the shared kernel WITHOUT adding a cell to the "
    "notebook (scratch evaluation, magics, shell). Returns outputs but leaves the "
    "notebook unchanged."
)
async def run_code(
    code: Annotated[str, Field(description="Code to execute")],
    timeout: Annotated[float, Field(description="Execution timeout in seconds")] = 120.0,
    path: Annotated[str | None, Field(description="Notebook whose kernel to use; defaults to the active notebook")] = None,
) -> ContentList:
    rel = _target(path)
    outputs, _ = await kernel.execute(rel, code, timeout)
    return kernel.outputs_to_mcp(outputs)


@mcp.tool(description="Restart the shared kernel for the active (or given) notebook (clears all in-memory state).")
async def kernel_restart(
    path: Annotated[str | None, Field(description="Notebook path; defaults to the active notebook")] = None,
) -> str:
    rel = _target(path)
    await kernel.restart_kernel(rel)
    return json.dumps({"restarted": rel})


@mcp.tool(
    description="Read-only semantic search over the shared `index` corpus (code "
    "plus Claude/Codex/shell history across the fleet). Scope with source, user, "
    "repo, host, project. Returns matching chunks as JSON."
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

    scope = _scope(source, user, repo, host, project)
    hits = await _search.semantic(query, top_k=top_k, **scope)
    return json.dumps(hits)


@mcp.tool(
    description="Read-only regex grep over the same shared `index` corpus the "
    "semantic search covers. Scope with source, user, repo, host, project."
)
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

    scope = _scope(source, user, repo, host, project)
    hits = await _search.grep(pattern, top_k=top_k, case_sensitive=case_sensitive, **scope)
    return json.dumps(hits)


def _scope(
    source: list[str] | None,
    user: list[str] | None,
    repo: str | None,
    host: list[str] | None,
    project: list[str] | None,
) -> dict[str, Any]:
    scope: dict[str, Any] = {}
    if source:
        scope["source"] = source
    if user:
        scope["user"] = user
    if repo:
        scope["repo"] = repo
    if host:
        scope["host"] = host
    if project:
        scope["project"] = project
    return scope


async def _run_cell_by_id(rel: str, ynb: Any, cell_id: str | None, timeout: float) -> ContentList:
    """Execute the cell with ``cell_id``, write outputs back into it, return them.

    Re-resolves the index by id right before reading the source and again before
    writing outputs, so a concurrent insert/delete by the human cannot make us
    run or overwrite the wrong cell.
    """
    if cell_id is None:
        raise ValueError("cell has no id")
    index = notebook.cell_index_by_id(ynb, cell_id)
    source = ynb.get_cell(index).get("source", "")
    try:
        outputs, execution_count = await kernel.execute(rel, source, timeout)
    except TimeoutError:
        index = notebook.cell_index_by_id(ynb, cell_id)
        kernel.write_outputs(
            ynb,
            index,
            [{"output_type": "error", "ename": "TimeoutError", "evalue": f"cell exceeded {timeout}s", "traceback": []}],
            None,
        )
        return [mcp_types.TextContent(type="text", text=f"cell timed out after {timeout}s")]
    index = notebook.cell_index_by_id(ynb, cell_id)
    kernel.write_outputs(ynb, index, outputs, execution_count)
    return kernel.outputs_to_mcp(outputs)
