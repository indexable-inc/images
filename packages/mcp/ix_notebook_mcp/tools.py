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

import asyncio
import contextlib
import json
import logging
import os
import sqlite3
import uuid
import weakref
from typing import Annotated

import mcp.types as types
from mcp.server.fastmcp import Context, FastMCP
from mcp.server.lowlevel.helper_types import ReadResourceContents
from mcp.shared.exceptions import McpError
from mcp.types import ErrorData
from pydantic import AnyUrl, Field

from . import guide, outputs, resources_bridge
from .config import config, server_version
from .kernel import current_kernel

logger = logging.getLogger(__name__)

# Order matters: clients truncate long instruction blocks from the tail, and a
# 2026-06-10 session showed exactly that failure: the cut landed inside JOBS, so
# the shell/data rules and POLARS never reached the model and it shelled out
# ls/grep and scraped TSV all session. The rules that shape every single call
# (what to reach for, what shape to return) come first; operational mechanics (job paging,
# blocking, rendering details) follow; the module index and dashboard niceties
# close. Losing the tail degrades gracefully; losing the head does not.
_KERNEL_GUIDE = guide.compose(
    guide.INTRO,
    guide.SESSION,
    guide.NAMESPACE,
    guide.DISCOVER,
    guide.NO_SHELL,
    guide.NU,
    guide.NIX,
    guide.SH,
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
    guide.CHANNEL,
)


mcp = FastMCP("ix-mcp")

# One short id per live MCP session, keyed weakly by the session object so an id
# is stable for a client's whole session and the map never pins a closed one.
_session_ids: weakref.WeakKeyDictionary = weakref.WeakKeyDictionary()


def _http_session(ctx: Context | None) -> object | None:
    """This call's live MCP session object under the HTTP transport, else None.

    Only the HTTP transport multiplexes several client sessions onto the one
    shared kernel, so only there is the session object a useful key. The stdio
    transport (and an embedder driving the tools directly, which has no config)
    serves exactly one client per process, reported here as None.
    """
    try:
        if config().transport != "http":
            return None
    except RuntimeError:
        # No config (an embedder driving the tools directly): single client.
        return None
    try:
        return ctx.session if ctx is not None else None
    except ValueError:
        # No request context on this call.
        return None


def _session_id(ctx: Context | None) -> str | None:
    """The kernel-side namespace key for this call's MCP session, or None.

    Each HTTP session gets its own namespace (the kernel runtime keys
    per-session globals on this id -- see ``runtime._session_ns``). The stdio
    transport's state stays in the shared user namespace, which is also what
    session checkpoint/restore (``serve --session FILE``) covers, so that
    contract is untouched.
    """
    session = _http_session(ctx)
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
_dashboard_started = False
# The label each client chose via `session_set_name`. A set label gates the
# acting tools (a session must name itself once); the labels are what the
# tailnet `/mesh` endpoint advertises (index#1787), read via `session_names`.
# HTTP sessions are keyed WEAKLY by the live session object (like
# `_session_ids` above), so a disconnected client's label vanishes with its
# session instead of a long-lived `serve --http` advertising it forever
# (index#1789 review). The one stdio/embedder client has no session object to
# key on; its label lives in `_solo_session_name` and dies with the process.
_session_labels: weakref.WeakKeyDictionary = weakref.WeakKeyDictionary()
_session_topics: weakref.WeakKeyDictionary = weakref.WeakKeyDictionary()
_solo_session_name: str | None = None
_solo_topic: str | None = None


def _session_label(ctx: Context | None) -> str | None:
    """The label this call's session chose, or None until it names itself."""
    session = _http_session(ctx)
    if session is None:
        return _solo_session_name
    return _session_labels.get(session)


def _set_session_label(ctx: Context | None, name: str) -> None:
    global _solo_session_name
    session = _http_session(ctx)
    if session is None:
        _solo_session_name = name
    else:
        _session_labels[session] = name




def _session_topic(ctx: Context | None) -> str | None:
    """The current fold topic this call's session chose."""
    session = _http_session(ctx)
    if session is None:
        return _solo_topic
    return _session_topics.get(session)


def _set_session_topic(ctx: Context | None, topic: str) -> None:
    global _solo_topic
    session = _http_session(ctx)
    if session is None:
        _solo_topic = topic
    else:
        _session_topics[session] = topic


def _topic_required() -> bool:
    """Whether python_exec requires an explicit topic first."""
    return os.environ.get("IX_MCP_REQUIRE_TOPIC", "1").strip().lower() not in (
        "0",
        "false",
        "no",
        "off",
    )


async def _require_topic(ctx: Context | None, *, intent: str | None = None) -> None:
    """Fail fast until this MCP session has named the current dashboard topic."""
    if not _topic_required() or _session_topic(ctx) is not None:
        return
    suggestion = f" Suggested topic from this call: {intent!r}." if intent else ""
    raise McpError(
        ErrorData(
            code=types.INVALID_REQUEST,
            message=(
                "Set a dashboard topic first: call topic_set with a short label for "
                "the current cluster of related tool calls."
                f"{suggestion}"
            ),
        )
    )

def session_names() -> list[str]:
    """The labels live MCP sessions gave themselves, for the mesh endpoint.

    Names only, sorted and deduplicated -- never code, outputs, or session
    keys. Labels set kernel-side (``session.name = ...`` in a cell) bypass this
    tool-side map and are deliberately not included: the mesh card is the
    server's own view of its clients (index#1787).
    """
    names = set(_session_labels.values())
    if _solo_session_name:
        names.add(_solo_session_name)
    return sorted(names)


async def _start_dashboard_once() -> None:
    """Best-effort first-tool dashboard startup.

    `serve` no longer owns a per-server hub, but a first real tool call is the
    moment a human expects the website to exist. Reuse the shared singleton so
    abnormal MCP exits do not leave one orphan hub per server.
    """
    global _dashboard_started
    if _dashboard_started:
        return
    _dashboard_started = True
    try:
        from .cli import ensure_shared_dashboard

        await asyncio.to_thread(ensure_shared_dashboard, open_browser=True)
    except Exception:
        logger.exception("dashboard autostart failed")


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


def _session_name_required() -> bool:
    """Whether acting tools must be preceded by ``session_set_name``."""
    return os.environ.get("IX_MCP_REQUIRE_SESSION_NAME", "1").strip().lower() not in (
        "0",
        "false",
        "no",
        "off",
    )


async def _require_session_name(ctx: Context | None, *, intent: str | None = None) -> None:
    """Fail fast until this MCP session has named its dashboard group."""
    if not _session_name_required() or _session_label(ctx) is not None:
        return
    suggestion = f" Suggested name from this call: {intent!r}." if intent else ""
    raise McpError(
        ErrorData(
            code=types.INVALID_REQUEST,
            message=(
                "Name this dashboard session first: call session_set_name with a "
                "short human task label before using acting tools."
                f"{suggestion}"
            ),
        )
    )


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
# can see exactly which commit of the server it is talking to. FastMCP does not
# take a version, so stamp the low-level server directly. The derivation lives
# in `config.server_version` so the `/mesh` endpoint reports the same value.
mcp._mcp_server.version = server_version()

Content = list[outputs.Content]


@mcp.tool(
    structured_output=False,
    description=(
        "Name this MCP connection's dashboard session. Call this before acting "
        "tools such as python_exec, read, kernel_trace, or tui_act; the name "
        "should be a short human task label, not code or secrets."
    ),
)
async def session_set_name(
    name: Annotated[
        str,
        Field(
            description=(
                "Short human task label for this dashboard session, 3 to 80 "
                "characters, with no code or secrets"
            )
        ),
    ],
    ctx: Context | None = None,
) -> Content:
    await _start_dashboard_once()
    await _identify_client_once(ctx)
    clean = " ".join((name or "").split())
    if not 3 <= len(clean) <= 80:
        raise McpError(
            ErrorData(
                code=types.INVALID_PARAMS,
                message="Session name must be 3 to 80 non-whitespace characters.",
            )
        )
    await current_kernel().set_session_name(clean)
    _set_session_label(ctx, clean)
    return [outputs.text(f"dashboard session named: {clean}")]


@mcp.tool(
    structured_output=False,
    description=(
        "Set the current dashboard topic for this MCP connection. Call this before "
        "a related cluster of python_exec calls, and change it when the work moves "
        "to a new phase; runs fold under the topic inside the session."
    ),
)
async def topic_set(
    topic: Annotated[
        str,
        Field(
            description=(
                "Short label for the current cluster of related tool calls, 3 to "
                "80 characters, with no code or secrets"
            )
        ),
    ],
    ctx: Context | None = None,
) -> Content:
    await _start_dashboard_once()
    await _identify_client_once(ctx)
    await _require_session_name(ctx, intent=topic)
    clean = " ".join((topic or "").split())
    if not 3 <= len(clean) <= 80:
        raise McpError(
            ErrorData(
                code=types.INVALID_PARAMS,
                message="Topic must be 3 to 80 non-whitespace characters.",
            )
        )
    await current_kernel().set_topic(clean)
    _set_session_topic(ctx, clean)
    return [outputs.text(f"dashboard topic set: {clean}")]


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
        guide.PR_WATCH,
        guide.SEE_INSTRUCTIONS,
    ),
)
async def python_exec(
    code: Annotated[str, Field(description="Python source to run on the shared kernel")],
    intent: Annotated[str, Field(description="Required. A short plain-language description of what this run does, e.g. 'count rows per host' or 'fetch and parse the open PR list'. It titles the run's card in the dashboard feed (grouped under your session) so a human watching can follow your work — never the raw code. Keep it under ~8 words.")],
    budget: Annotated[float, Field(description="Seconds to wait before backgrounding the run (server-side cap: 120s; larger values are clamped and a notice is appended to the reply)")] = 15.0,
    ctx: Context | None = None,
) -> Content:
    await _start_dashboard_once()
    await _identify_client_once(ctx)
    await _require_session_name(ctx, intent=intent)
    await _require_topic(ctx, intent=intent)
    # A foreground budget is how long the run holds the one shared shell channel
    # before it backgrounds, so cap it: a giant budget (a 15-minute `await
    # jobs[...]`) would block every other call behind it. The clamp is surfaced
    # below so the caller knows to poll the job rather than silently lose the wait.
    cap = config().max_budget
    effective_budget = min(budget, cap)
    # `intent` is the run's human label (the dashboard feed's title); it flows to
    # the kernel as the job name and lands in the store's `name` column.
    cell_outputs, summary = await current_kernel().python_exec(
        code, effective_budget, intent, session=_session_id(ctx), topic=_session_topic(ctx)
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


@mcp.tool(
    structured_output=False,
    description=(
        "Watch a GitHub pull request in the dashboard. Creates a live PR resource "
        "nested under this task, lists required checks and actions with elapsed "
        "time, enables auto merge by default, and notifies the CLI when the PR "
        "merges, fails, or times out. Use this instead of hand-written PR polling."
    ),
)
async def pr_watch(
    pr: Annotated[
        str,
        Field(description="PR number, URL, or branch understood by gh, for example 1856."),
    ],
    cwd: Annotated[
        str,
        Field(description="Repository worktree where gh should run."),
    ],
    *,
    auto_merge: Annotated[
        bool,
        Field(description="Enable gh auto merge for this PR before watching."),
    ] = True,
    interval: Annotated[
        float,
        Field(description="Seconds between GitHub status refreshes."),
    ] = 15.0,
    timeout: Annotated[
        float,
        Field(description="Seconds to watch before the resource closes as timed out."),
    ] = 3600.0,
    ctx: Context | None = None,
) -> Content:
    await _start_dashboard_once()
    await _identify_client_once(ctx)
    await _require_session_name(ctx, intent=f"watch PR {pr}")
    await _require_topic(ctx, intent=f"watch PR {pr}")
    code = (
        "await watch_pr("
        f"{pr!r}, cwd={cwd!r}, auto_merge={auto_merge!r}, "
        f"interval={interval!r}, timeout={timeout!r}"
        ")"
    )
    cell_outputs, summary = await current_kernel().python_exec(
        code,
        min(5.0, config().max_budget),
        f"watch PR {pr}",
        session=_session_id(ctx),
        topic=_session_topic(ctx),
    )
    rendered = outputs.to_mcp(cell_outputs)
    resource = f"pr-{re.sub(r'[^A-Za-z0-9._-]+', '-', str(pr)).strip('-')}"
    header = outputs.text(
        json.dumps(
            {
                "job": summary.get("id") if summary else None,
                "status": summary.get("status") if summary else None,
                "running": summary.get("running") if summary else None,
                "elapsed_s": summary.get("elapsed_s") if summary else None,
                "resource": resource,
            }
        )
    )
    return [header, *(item for item in rendered if getattr(item, "text", None) != "(no output)")]


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
    await _start_dashboard_once()
    await _identify_client_once(ctx)
    await _require_session_name(ctx, intent=f"read {target}")
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
async def kernel_trace(ctx: Context | None = None) -> str:
    await _start_dashboard_once()
    await _identify_client_once(ctx)
    await _require_session_name(ctx, intent="kernel trace")
    return await current_kernel().dump_trace()


# The reply tool's store connection, opened lazily on first reply. The tool runs
# in the server process (the kernel's `events` writes come from its own
# connection), so it needs its own handle on the shared WAL store.
_reply_conn: sqlite3.Connection | None = None


def _reply_store() -> sqlite3.Connection:
    global _reply_conn
    if _reply_conn is None:
        try:
            path = config().store_path
        except RuntimeError as exc:
            raise McpError(
                ErrorData(code=types.INTERNAL_ERROR, message="no store configured; reply needs `ix-mcp serve`")
            ) from exc
        from . import store

        _reply_conn = store.connect(path)
    return _reply_conn


@mcp.tool(structured_output=False, description=guide.REPLY)
async def reply(
    resource: Annotated[
        str,
        Field(description='The resource id from the channel event\'s resource="..." attribute'),
    ],
    text: Annotated[str, Field(description="The message to show on that resource's page")],
    ctx: Context | None = None,
) -> Content:
    await _start_dashboard_once()
    await _identify_client_once(ctx)
    # Deliberately not gated on session_set_name: a reply answers a channel event
    # (often the session's very first act) and creates no dashboard run to label.
    from . import store

    conn = _reply_store()
    if not store.resource_live(conn, resource):
        raise McpError(
            ErrorData(
                code=types.INVALID_PARAMS,
                message=f"no live resource {resource!r}; pass the id from the <channel resource=...> attribute",
            )
        )
    store.add_event(conn, resource=resource, kind="reply", body=json.dumps({"text": text}))
    return [outputs.text("sent")]


# ---------------------------------------------------------------------------
# Federated TUI resources: resources/list + resources/read + the tui_act tool.
# ---------------------------------------------------------------------------
#
# The federated terminal resources another node advertises are bridged in through
# the `ix` CLI (see resources_bridge). They surface here as real MCP resources so
# a client can `@`-mention one, plus a `tui_act` tool so an agent can drive it.
#
# FastMCP's own resources/list only returns statically-registered resources (its
# ResourceManager), with no hook for a list discovered at runtime -- so the
# federated list is served by registering low-level handlers directly on the
# wrapped server (`mcp._mcp_server`). These OVERRIDE the handlers FastMCP wired at
# construction, so each delegates back to FastMCP's static path and then folds in
# the federated entries, keeping any `@mcp.resource` registration working too.


async def _list_resources_handler() -> list[types.Resource]:
    """Serve `resources/list`: FastMCP's static resources plus federated ones.

    Federated discovery degrades gracefully (an absent/unhealthy `ix` yields an
    empty federated list), so this always returns at least the static set.
    """
    static = await mcp.list_resources()
    federated: list[types.Resource] = []
    # A broad catch is deliberate: discovery is best-effort, so any failure
    # (CLI, network, parse) must degrade to the static set, never fail the request.
    try:
        federated = [
            types.Resource(
                uri=AnyUrl(entry.uri),
                name=entry.name or entry.uri,
                description=_federated_description(entry),
                mimeType=entry.mime or "text/plain",
            )
            for entry in await resources_bridge.list_resources()
        ]
    except Exception:
        logger.exception("federated resources/list failed; returning static only")
    return [*static, *federated]


def _federated_description(entry: resources_bridge.ResourceEntry) -> str:
    """A one-line human label for a federated resource card."""
    caps = ", ".join(entry.caps) if entry.caps else "—"
    state = "alive" if entry.alive else "dead"
    host = entry.host or _uri_host(entry.uri) or "?"
    return f"federated TUI resource on {host} ({state}; caps: {caps})"


def _uri_host(uri: str) -> str | None:
    prefix = "ix://"
    if not uri.startswith(prefix):
        return None
    return uri[len(prefix) :].partition("/")[0] or None


async def _read_resource_handler(uri: AnyUrl) -> list[ReadResourceContents]:
    """Serve `resources/read`: try FastMCP's static resources, then the federation.

    A uri unknown to both raises an `McpError` carrying the resources spec's
    `-32002` (resource-not-found) code so the client gets a precise error.
    """
    uri_str = str(uri)
    # Static FastMCP resources first (a hand-registered `@mcp.resource`), so the
    # federation only handles uris FastMCP does not own. The manager RAISES
    # ValueError for an unknown uri (it does not return None), so treat that as
    # "not static" and fall through to the federation rather than erroring.
    try:
        resource = await mcp._resource_manager.get_resource(uri_str, context=mcp.get_context())
    except ValueError:
        resource = None
    if resource is not None:
        content = await resource.read()
        return [ReadResourceContents(content=content, mime_type=resource.mime_type)]
    try:
        text, mime = await resources_bridge.read_resource(uri_str)
    except resources_bridge.ResourceNotFoundError as exc:
        raise McpError(ErrorData(code=resources_bridge.RESOURCE_NOT_FOUND, message=str(exc))) from exc
    except resources_bridge.ResourceBridgeError as exc:
        raise McpError(ErrorData(code=types.INTERNAL_ERROR, message=str(exc))) from exc
    return [ReadResourceContents(content=text, mime_type=mime)]


# Register on the wrapped low-level server (overriding FastMCP's, which the
# handlers above delegate back to for static resources).
mcp._mcp_server.list_resources()(_list_resources_handler)
mcp._mcp_server.read_resource()(_read_resource_handler)


@mcp.tool(
    structured_output=False,
    description=(
        "Drive a federated TUI resource: send keystrokes to a peer's live terminal "
        "resource (one you can `@`-mention from resources/list) and return the "
        "peer's acknowledgement. `uri` is the resource's `ix://<host>/<name>` uri; "
        "`send_keys` is the literal key sequence to deliver (e.g. 'ls\\n', "
        "'C-c'). `peer` is an optional full endpoint URL (e.g. "
        "'https://<addr>/rpc') targeting one peer directly; omit it to probe the "
        "configured peers (IX_RESOURCE_PEERS) for the one advertising the uri. "
        "Bridges to `ix-resource-cli act` and degrades clearly when the CLI is absent."
    ),
)
async def tui_act(
    uri: Annotated[str, Field(description="The ix://<host>/<name> uri of the federated resource to drive")],
    send_keys: Annotated[str, Field(description="Literal keystrokes to send to the resource, e.g. 'ls\\n' or 'C-c'")],
    peer: Annotated[
        str | None,
        Field(description="Optional full endpoint URL (e.g. 'https://<addr>/rpc') of one peer to target; omit to probe the configured peers for the uri's owner"),
    ] = None,
    ctx: Context | None = None,
) -> Content:
    await _start_dashboard_once()
    await _identify_client_once(ctx)
    await _require_session_name(ctx, intent=f"drive {uri}")
    try:
        ack = await resources_bridge.act(uri, send_keys, peer=peer)
    except resources_bridge.ResourceNotFoundError as exc:
        raise McpError(ErrorData(code=resources_bridge.RESOURCE_NOT_FOUND, message=str(exc))) from exc
    except resources_bridge.ResourceBridgeError as exc:
        return [outputs.text(f"tui_act failed: {exc}")]
    return [outputs.text(json.dumps(ack))]


# Seed the server instructions now that every `@mcp.tool` above is registered, so
# the tool overview is derived from the registry rather than maintained by hand.
# `set_dashboard_url` re-composes them with the live dashboard URL before serving.
mcp._mcp_server.instructions = _compose_instructions()
