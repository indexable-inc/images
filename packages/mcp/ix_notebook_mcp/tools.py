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

import contextlib
import json
import os
import uuid
import weakref
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
    guide.SESSION,
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
    guide.OUTPUT_HTML,
    guide.VERIFY,
    guide.AUTOMERGE,
    guide.RESULT_SPLIT,
    guide.RESULT_VARIANTS,
    guide.READABLE,
    guide.CELLS,
)


mcp = FastMCP("ix-mcp")

# One short id per live MCP session, keyed weakly by the session object so an id
# is stable for a client's whole session and the map never pins a closed one.
_session_ids: weakref.WeakKeyDictionary = weakref.WeakKeyDictionary()


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


# Set once the connecting client has been identified to the kernel, so the
# session label defaults to it (see runtime.Session). A latch, not per-call work.
_client_identified = False


def _client_label(ctx: Context | None) -> str:
    """The connecting MCP client's identity (name + version), from the
    ``initialize`` handshake, or "" when unavailable. This is what the session
    label defaults to so a human can tell one agent's runs from another's."""
    try:
        session = ctx.session if ctx is not None else None
    except ValueError:
        # No request context on this call (an embedder driving the tools directly).
        return ""
    params = getattr(session, "client_params", None) if session is not None else None
    info = getattr(params, "clientInfo", None) if params is not None else None
    if info is None:
        return ""
    name = (getattr(info, "name", "") or "").strip()
    version = (getattr(info, "version", "") or "").strip()
    if name and version:
        return f"{name} {version}"
    return name or version


async def _identify_client_once(ctx: Context | None) -> None:
    """Tell the kernel which client connected, once per server process, so an
    unnamed session still reads as e.g. ``claude-code · index`` rather than an
    opaque id. Best-effort: the kernel call swallows its own failures.

    Latches only once a label is actually resolved, so a first call that lacks
    ``clientInfo`` (no request context yet) does not permanently suppress a later
    call that has it. The label lookup is sync and cheap, so retrying costs
    nothing; the latch is still set before the first ``await`` so a concurrent
    first call cannot double-fire."""
    global _client_identified
    if _client_identified:
        return
    label = _client_label(ctx)
    if not label:
        return
    _client_identified = True  # latch before awaiting so a concurrent first call skips
    with contextlib.suppress(Exception):  # no kernel yet or transient error: label is a convenience
        await current_kernel().set_client(label)


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
    lines.extend(f"- `{tool.name}`: {_first_sentence(tool.description)}." for tool in mcp._tool_manager.list_tools())
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
    The URL is stashed so a tool call can surface it, never auto-popped in a
    browser. The human-facing UI is the standalone aggregator (`nix run
    .#dashboard`), which renders every server at once.
    """
    global _dashboard_url
    _dashboard_url = url
    mcp._mcp_server.instructions = _compose_instructions(url)


# The live dashboard URL (set by `set_dashboard_url`). Module-level because the
# tool functions are module-level.
_dashboard_url: str | None = None

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
    intent: Annotated[str, Field(description="Required. A short plain-language description of what this run does, e.g. 'count rows per host' or 'fetch and parse the open PR list'. It titles the run's card in the dashboard feed (grouped under your session) so a human watching can follow your work — never the raw code. Keep it under ~8 words.")],
    budget: Annotated[float, Field(description="Seconds to wait before backgrounding the run (server-side cap: 120s; larger values are clamped and a notice is appended to the reply)")] = 15.0,
    ctx: Context | None = None,
) -> Content:
    await _identify_client_once(ctx)
    # A foreground budget is how long the run holds the one shared shell channel
    # before it backgrounds, so cap it: a giant budget (a 15-minute `await
    # jobs[...]`) would block every other call behind it. The clamp is surfaced
    # below so the caller knows to poll the job rather than silently lose the wait.
    cap = config().max_budget
    effective_budget = min(budget, cap)
    # `intent` is the run's human label (the dashboard feed's title); it flows to
    # the kernel as the job name and lands in the store's `name` column.
    cell_outputs, summary = await current_kernel().python_exec(
        code, effective_budget, intent, session=_session_id(ctx)
    )
    rendered = outputs.to_mcp(cell_outputs)
    if summary is None:
        return rendered
    header = outputs.text(
        json.dumps(
            {
                "job": summary.get("id"),
                "status": summary.get("status"),
                "running": summary.get("running"),
                # Wall-clock cost of this run, reported by default so the caller
                # notices a slow run (the house system prompt's kernel-timing rule
                # says to treat one as a problem to fix, not an FYI).
                "elapsed_s": summary.get("elapsed_s"),
            }
        )
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
    await _identify_client_once(ctx)
    sid = _session_id(ctx)
    code = f"await __ix_read({target!r}, {start!r}, {end!r}, session={sid!r})"
    # Title the run by what it reads (with the line span when given) so its card
    # reads "read path/to/file.py:10-40", not the raw `await __ix_read(...)` call.
    span = f":{start}-{end}" if start is not None and end is not None else (f":{start}" if start is not None else "")
    name = f"read {target}{span}"
    cell_outputs, summary = await current_kernel().python_exec(code, budget=30.0, name=name, session=sid)
    if summary is not None and summary.get("status") == "error" and summary.get("error"):
        return [outputs.text(summary["error"])]
    rendered = outputs.to_mcp(cell_outputs)
    content = [item for item in rendered if getattr(item, "text", None) != "(no output)"]
    return content or rendered


@mcp.tool(structured_output=False, description=guide.TRACE)
async def kernel_trace() -> str:
    return await current_kernel().dump_trace()


# Seed the server instructions now that every `@mcp.tool` above is registered, so
# the tool overview is derived from the registry rather than maintained by hand.
# `set_dashboard_url` re-composes them with the live dashboard URL before serving.
mcp._mcp_server.instructions = _compose_instructions()
