"""The MCP tool surface.

``python_exec`` is the one general tool: it runs code on the single shared kernel
with a foreground budget and, if the work outlives the budget, leaves it running
in the background as an entry in the in-kernel ``jobs`` dict. Job control needs no
extra tools because ``jobs`` is just namespace state: inspect/await/cancel it with
more ``python_exec`` (``jobs['ab12'].cancel()``). Everything else an agent might
want (search the index, read the calendar, shell out) is reachable the same way,
by importing the bundled module inside a cell, so it does not earn a dedicated
tool.

Two tools earn their place beside it because they do something ``python_exec``
cannot. ``read`` pulls a file or a kernel value into the MODEL's context without
spamming the human: the full text comes back to the agent while the dashboard
shows only a one-line note. A plain cell cannot make that split for free — its
result streams to both audiences — so reading a large file or paging a job's
output through ``python_exec`` either floods the dashboard or costs the human a
wall of text they did not ask for. ``kernel_trace`` dumps the kernel's stack out
of band (a faulthandler signal, not the execute channel) so it works even when a
cell has wedged the event loop, which is exactly when ``python_exec`` cannot help.

The server ``instructions`` a client reads at ``initialize`` are composed, not
hand-listed: :func:`_compose_instructions` joins the authored ``_KERNEL_GUIDE``
with a tool overview derived from the registry (:func:`_tools_overview`) and,
once the dashboard has a port, its live URL. So each ``@mcp.tool`` describes
itself once and lists itself in the instructions automatically -- nothing here
restates a tool by hand.
"""

from __future__ import annotations

import json
import os
from typing import Annotated

from mcp.server.fastmcp import FastMCP
from pydantic import Field

from . import guide, outputs
from .kernel import current_kernel

_KERNEL_GUIDE = guide.compose(
    guide.INTRO,
    guide.NAMESPACE,
    guide.DISCOVER,
    guide.JOBS,
    guide.PAGING,
    guide.BLOCKING,
    guide.MODULES,
    guide.HTML,
    guide.VIEW,
    guide.POLARS,
    guide.SEARCH,
    guide.RESULT_CONTRACT,
    guide.RESULT_SPLIT,
    guide.RESULT_VARIANTS,
    guide.READABLE,
    guide.CELLS,
)


mcp = FastMCP("ix-mcp")


def _first_sentence(text: str) -> str:
    """The lead clause of a tool's description, for the one-line tool overview the
    server instructions build from the registry. Cuts at the earliest sentence
    break so each tool contributes a single tidy summary, never its whole body."""
    lead = " ".join((text or "").split())
    breaks = (lead.find(sep) for sep in (". ", ": ", "; ", "? ", "! "))
    cut = min((i for i in breaks if i != -1), default=len(lead))
    return lead[:cut].rstrip(".:;?!")


def _tools_overview() -> str:
    """A one-line-per-tool overview DERIVED from the registered MCP tools, so the
    instructions never restate by hand what each tool's own description already
    says: register a `@mcp.tool` and it lists itself here automatically."""
    lines = ["The MCP tools you can call (each carries its own fuller description):"]
    for tool in mcp._tool_manager.list_tools():
        lines.append(f"- `{tool.name}`: {_first_sentence(tool.description)}.")
    return "\n".join(lines)


def _compose_instructions(dashboard_url: str | None = None) -> str:
    """The full server instructions: the kernel guide, then the registry-derived
    tool overview, then (once the dashboard has bound a port) its live URL. Called
    at import to seed the instructions and again by `set_dashboard_url` to fold the
    URL in before the transport serves ``initialize``."""
    parts = [_KERNEL_GUIDE, _tools_overview()]
    if dashboard_url:
        parts.append(guide.dashboard_note(dashboard_url))
    return "\n\n".join(parts)


def set_dashboard_url(url: str) -> None:
    """Bake the live dashboard URL into the server instructions so a client reads
    it straight out of the ``initialize`` response -- the agent has the URL from
    the first message, with no tool call to look it up. The CLI calls this once
    the dashboard has bound its port, before the transport serves ``initialize``.
    """
    mcp._mcp_server.instructions = _compose_instructions(url)

# Report the build's source revision as the MCP `serverInfo.version` so a client
# can see exactly which commit of the server it is talking to. The nix wrapper
# sets `IX_MCP_VERSION` to the flake rev (`<commit>` / `<commit>-dirty` / "dev");
# FastMCP does not take a version, so stamp the low-level server directly. Absent
# the env var (a bare `python -m ix_notebook_mcp`) it falls back to "dev".
mcp._mcp_server.version = os.environ.get("IX_MCP_VERSION") or "dev"

Content = list[outputs.Content]


@mcp.tool(
    description=guide.compose(
        guide.PYEXEC_INTRO,
        guide.PAGING,
        guide.NAMESPACE,
        guide.BLOCKING,
        guide.RESULT_CONTRACT,
        guide.SEE_INSTRUCTIONS,
    )
)
async def python_exec(
    code: Annotated[str, Field(description="Python source to run on the shared kernel")],
    budget: Annotated[float, Field(description="Seconds to wait before backgrounding the run")] = 15.0,
    name: Annotated[str | None, Field(description="Optional label for the job in the dashboard")] = None,
) -> Content:
    cell_outputs, summary = await current_kernel().python_exec(code, budget, name)
    rendered = outputs.to_mcp(cell_outputs)
    if summary is None:
        return rendered
    header = outputs.text(
        json.dumps({"job": summary.get("id"), "status": summary.get("status"), "running": summary.get("running")})
    )
    parts: Content = [header]
    # Print is not a channel back to the model: a job's stdout is captured for the
    # dashboard (collapsed) and for paging via jobs['<id>'].output, but it is not
    # returned here \u2014 results come from Result/yield. A failing run is the
    # exception: its traceback IS the result, so surface that.
    if summary.get("status") == "error" and summary.get("error"):
        parts.append(outputs.text(summary["error"]))
    # Rich result blocks (images / HTML / the result repr, including every yielded
    # Result) come from the kernel display; drop the "(no output)" placeholder
    # to_mcp emits when there were none.
    parts.extend(item for item in rendered if getattr(item, "text", None) != "(no output)")
    # When the reply was clipped to fit, the full run still lives in the kernel as
    # jobs['<id>']. Point the caller at it (with the ops to page it) so a large
    # result is recoverable without re-running the work \u2014 the failure mode this
    # whole jobs registry exists to avoid.
    job_id = summary.get("id")
    output_chars = summary.get("output_chars") or 0
    result_chars = summary.get("result_chars") or 0
    clipped = result_chars > outputs.MAX_TEXT_CHARS
    if clipped and job_id:
        parts.append(
            outputs.text(
                f"[Reply truncated to fit. The full run stays in this kernel as "
                f"jobs['{job_id}'] (stdout {output_chars} chars, result {result_chars} chars). "
                f"Page it in a new python_exec cell instead of re-running: "
                f"Result.text(jobs['{job_id}'].grep('pattern')) | .tail(8000) | .head(8000) | "
                f".slice(50000, 70000) | .lines(0, 200). jobs['{job_id}'].output is the full "
                f"stdout, jobs['{job_id}'].result the value; history() lists recent runs.]"
            )
        )
    return parts


@mcp.tool(description=guide.READ)
async def read(
    target: Annotated[
        str,
        Field(
            description=(
                "A file path, or a Python expression evaluated in the kernel "
                "(e.g. jobs['ab12'].output, or a variable you bound earlier)"
            )
        ),
    ],
    start: Annotated[int | None, Field(description="1-based first line to include")] = None,
    end: Annotated[int | None, Field(description="Last line to include (inclusive)")] = None,
) -> Content:
    code = f"await __ix_read({target!r}, {start!r}, {end!r})"
    cell_outputs, summary = await current_kernel().python_exec(code, budget=30.0)
    if summary is not None and summary.get("status") == "error" and summary.get("error"):
        return [outputs.text(summary["error"])]
    rendered = outputs.to_mcp(cell_outputs)
    content = [item for item in rendered if getattr(item, "text", None) != "(no output)"]
    return content or rendered


@mcp.tool(description=guide.TRACE)
async def kernel_trace() -> str:
    return await current_kernel().dump_trace()


# Seed the server instructions now that every `@mcp.tool` above is registered, so
# the tool overview is derived from the registry rather than maintained by hand.
# `set_dashboard_url` re-composes them with the live dashboard URL before serving.
mcp._mcp_server.instructions = _compose_instructions()
