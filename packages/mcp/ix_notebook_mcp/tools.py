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
import threading
import uuid
import weakref
import webbrowser
from typing import Annotated

from mcp.server.fastmcp import Context, FastMCP
from pydantic import Field

from . import guide, outputs
from .config import config
from .kernel import current_kernel

# Order matters: clients truncate long instruction blocks from the tail, and a
# 2026-06-10 session showed exactly that failure: the cut landed inside JOBS, so
# NO_SHELL and POLARS never reached the model and it shelled out ls/grep and
# scraped TSV all session. The rules that shape every single call (what to reach
# for, what shape to return) come first; operational mechanics (job paging,
# blocking, rendering details) follow; the module index and dashboard niceties
# close. Losing the tail degrades gracefully; losing the head does not.
_KERNEL_GUIDE = guide.compose(
    guide.INTRO,
    guide.NAMESPACE,
    guide.DISCOVER,
    guide.NO_SHELL,
    guide.POLARS,
    guide.RESULT_CONTRACT,
    guide.JOBS,
    guide.PAGING,
    guide.BLOCKING,
    guide.modules_index(),
    guide.credentials_note(),
    guide.HTML,
    guide.VERIFY,
    guide.RESULT_SPLIT,
    guide.RESULT_VARIANTS,
    guide.READABLE,
    guide.CELLS,
)


mcp = FastMCP("ix-mcp")

# One short id per live MCP session, keyed weakly by the session object so an id
# is stable for a client's whole session and the map never pins a closed one.
_session_ids: "weakref.WeakKeyDictionary" = weakref.WeakKeyDictionary()


def _session_id(ctx: Context | None) -> str | None:
    """The kernel-side namespace key for this call's MCP session, or None.

    Only the HTTP transport multiplexes several client sessions onto the one
    shared kernel, so only there does each session get its own namespace (the
    kernel runtime keys per-session globals on this id -- see
    ``runtime._session_ns``). The stdio transport serves exactly one client per
    process: its state stays in the shared user namespace, which is also what
    session checkpoint/restore (``serve --session FILE``) covers, so that
    contract is untouched.
    """
    try:
        if config().transport != "http":
            return None
    except RuntimeError:
        # No config (an embedder driving the tools directly): single client.
        return None
    try:
        session = ctx.session if ctx is not None else None
    except ValueError:
        # No request context on this call.
        session = None
    if session is None:
        return None
    sid = _session_ids.get(session)
    if sid is None:
        sid = uuid.uuid4().hex[:8]
        _session_ids[session] = sid
    return sid


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
    The URL is also stashed so the first tool call can pop it in a browser.
    """
    global _dashboard_url
    _dashboard_url = url
    mcp._mcp_server.instructions = _compose_instructions(url)


# The live dashboard URL (set by `set_dashboard_url`) and a once-latch for the
# browser pop below. Module-level because the tool functions are module-level.
_dashboard_url: str | None = None
_browser_opened = False


def _open_dashboard_once() -> None:
    """Open the live dashboard in the human's browser on the FIRST tool call.

    The server is launched eagerly when a client session starts, so opening at
    startup would pop a window for sessions that never touch index; the first
    tool call is the moment work actually begins. ``webbrowser`` is the
    platform-independent opener (macOS ``open``, Linux ``xdg-open``, Windows
    ``start``) and degrades to a no-op where no browser is reachable (headless,
    SSH); failures are swallowed because the pop is a courtesy, never worth
    failing a tool call over. The call runs on a daemon thread since spawning
    the opener is synchronous and must not block the event loop. An embedder
    that drives this server programmatically (no human at this machine's
    display) disables it with ``IX_MCP_NO_BROWSER=1``.
    """
    global _browser_opened
    if _browser_opened:
        return
    _browser_opened = True
    url = _dashboard_url
    if not url or os.environ.get("IX_MCP_NO_BROWSER"):
        return

    def _open() -> None:
        try:
            webbrowser.open(url)
        except Exception:
            pass

    threading.Thread(target=_open, name="ix-mcp-open-dashboard", daemon=True).start()

# Report the build's source revision as the MCP `serverInfo.version` so a client
# can see exactly which commit of the server it is talking to. The nix wrapper
# sets `IX_MCP_VERSION` to the flake rev (`<commit>` / `<commit>-dirty` / "dev");
# FastMCP does not take a version, so stamp the low-level server directly. Absent
# the env var (a bare `python -m ix_notebook_mcp`) it falls back to "dev".
mcp._mcp_server.version = os.environ.get("IX_MCP_VERSION") or "dev"

Content = list[outputs.Content]


# Every tool sets structured_output=False: FastMCP otherwise derives an output
# schema from the return annotation and DUPLICATES the entire reply as
# `structuredContent` JSON, so each image block went to the client twice (once
# as a real image, once as a wall of base64-in-text), which is what kept
# blowing the host's per-result token cap. The content blocks ARE the reply;
# there is no structured consumer.
@mcp.tool(
    structured_output=False,
    description=guide.compose(
        guide.PYEXEC_INTRO,
        guide.PAGING,
        guide.NAMESPACE,
        guide.BLOCKING,
        guide.RESULT_CONTRACT,
        guide.SEE_INSTRUCTIONS,
    ),
)
async def python_exec(
    code: Annotated[str, Field(description="Python source to run on the shared kernel")],
    budget: Annotated[float, Field(description="Seconds to wait before backgrounding the run (server-side cap: 120s; larger values are clamped and a notice is appended to the reply)")] = 15.0,
    name: Annotated[str | None, Field(description="Optional label for the job in the dashboard")] = None,
    ctx: Context | None = None,
) -> Content:
    _open_dashboard_once()
    # A foreground budget is how long the run holds the one shared shell channel
    # before it backgrounds, so cap it: a giant budget (a 15-minute `await
    # jobs[...]`) would block every other call behind it. The clamp is surfaced
    # below so the caller knows to poll the job rather than silently lose the wait.
    cap = config().max_budget
    effective_budget = min(budget, cap)
    cell_outputs, summary = await current_kernel().python_exec(
        code, effective_budget, name, session=_session_id(ctx)
    )
    rendered = outputs.to_mcp(cell_outputs)
    if summary is None:
        return rendered
    header = outputs.text(
        json.dumps({"job": summary.get("id"), "status": summary.get("status"), "running": summary.get("running")})
    )
    parts: Content = [header]
    # The kernel folds a cell's stdout into its result (Jupyter semantics; see
    # runtime._merge_stdout/_auto_result), so the rendered blocks below already
    # carry what the cell printed. A failing run's traceback IS its result, so
    # surface that here.
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
    # The requested budget exceeded the cap, so the run was given the smaller
    # foreground window and (if it outlived it) backgrounded. Tell the caller so a
    # long wait is resumed by polling the job, not mistaken for a finished run.
    if budget > cap and job_id:
        parts.append(
            outputs.text(
                f"[budget {budget:g}s exceeds the {cap:g}s cap and was clamped to "
                f"{cap:g}s: a foreground call holds the kernel's one shell channel, so "
                f"a longer wait backgrounds instead of blocking every other call. If "
                f"jobs['{job_id}'] is still running, resume it with await jobs['{job_id}'] "
                f"(or poll jobs['{job_id}'].done()) in a later cell.]"
            )
        )
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


@mcp.tool(structured_output=False, description=guide.READ)
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
    ctx: Context | None = None,
) -> Content:
    _open_dashboard_once()
    sid = _session_id(ctx)
    code = f"await __ix_read({target!r}, {start!r}, {end!r}, session={sid!r})"
    cell_outputs, summary = await current_kernel().python_exec(code, budget=30.0, session=sid)
    if summary is not None and summary.get("status") == "error" and summary.get("error"):
        return [outputs.text(summary["error"])]
    rendered = outputs.to_mcp(cell_outputs)
    content = [item for item in rendered if getattr(item, "text", None) != "(no output)"]
    return content or rendered


@mcp.tool(structured_output=False, description=guide.TRACE)
async def kernel_trace() -> str:
    _open_dashboard_once()
    return await current_kernel().dump_trace()


# Seed the server instructions now that every `@mcp.tool` above is registered, so
# the tool overview is derived from the registry rather than maintained by hand.
# `set_dashboard_url` re-composes them with the live dashboard URL before serving.
mcp._mcp_server.instructions = _compose_instructions()
