"""The MCP tool surface.

One execution tool, ``python_exec``: it runs code on the single shared kernel
with a foreground budget and, if the work outlives the budget, leaves it running
in the background as an entry in the in-kernel ``jobs`` dict. Job control needs
no extra tools because ``jobs`` is just namespace state: inspect/await/cancel it
with more ``python_exec`` (``jobs['ab12'].cancel()``). ``search_*`` and
``calendar_*`` stay as thin convenience tools over the bundled integrations.
"""

from __future__ import annotations

import asyncio
import json
import os
from typing import Annotated, Any

from mcp.server.fastmcp import FastMCP
from pydantic import Field

from . import outputs
from .kernel import current_kernel

mcp = FastMCP(
    "ix-mcp",
    instructions=(
        "Run Python on one shared, persistent kernel with `python_exec`. The "
        "namespace persists across calls, so variables, functions, classes, and "
        "imports you define stay defined and are reusable by every later call \u2014 "
        "define a helper once and call it again next turn. "
        "Each call runs as an async task and waits up to `budget` seconds; if the "
        "work is still going it keeps running in the background and the call "
        "returns a job handle. Background jobs live in the `jobs` dict, so manage "
        "them with more python_exec: `jobs['ab12']` to inspect, `await "
        "jobs['ab12']` to wait, `jobs['ab12'].cancel()` to stop, "
        "`[j for j in jobs.values() if j.running()]` to list. Many jobs run at "
        "once and none blocks the others, but only if you never block the one "
        "shared event loop. Any synchronous wait (`subprocess.run`, `time.sleep`, "
        "`requests`, a long CPU-bound numpy op) freezes the WHOLE kernel: every "
        "other job and even your own next status-check cell stall behind it until "
        "it returns. So a blocking call MUST be made non-blocking: wrap it in "
        "`await asyncio.to_thread(...)`, or prefer the async API (`fff`, `httpx`, "
        "`asyncio.create_subprocess_exec`), and run anything slow as a "
        "background job you poll, never inline. Bundled modules import with no install step: `fff` "
        "(async file search/grep), `view`, `tui`, `search`, `exa_py`, "
        "`google_auth`, `fleet` (async polars SSH fan-out across hosts: "
        "`await fleet.read_ndjson(hosts, path)`), numpy, polars, duckdb, "
        "httpx, matplotlib, playwright. "
        "Prefer these over shelling out: `view.ls/tree/grep/find` return "
        "polars DataFrames (composable + a styled HTML table) and "
        "`view.cat/read/head/json/diff` return a syntax-highlighted view, so "
        "you never hand-roll display HTML or run `ls`/`grep`/`cat` in bash. "
        "Prefer polars for any tabular data: return a DataFrame (or "
        "`Result.of(df)`) and the human gets the styled HTML table for free "
        "while you get the frame as compact, untruncated CSV \u2014 so you never "
        "hand-build a table and a wide/long-stringed frame is never clipped to "
        "you. Use `pl`; pandas is not bundled. "
        "Prefer `fff` for searching the working tree over shelling out to "
        "find/grep/rg: `fff.find(query)` (typo-tolerant, frecency-ranked file "
        "find) and `fff.grep(pattern)` / async `fff.afind` / `fff.agrep` "
        "(SIMD content grep) return results that render as the same styled "
        "table (`.df` for the frame) and give you compact text. "
        "EVERY cell MUST END with a `Result(...)`; the kernel rejects a cell whose "
        "last expression is not one (a bare value or a side-effect that returns "
        "nothing fails with a reminder). A Result splits the human view from your "
        "view: the dashboard renders `user_html` for the human while your tool "
        "result gets `llm_result` text plus any `llm_images`, so a big rendered "
        "view never costs you tokens and you can hand yourself back images. Use the "
        "shortcuts: `Result.text('done')` (same text to both), `Result.ok('what "
        "happened')` (a quiet confirmation for a side-effecting cell), "
        "`Result.of(df)` (render any value richly for the human, its repr to you), "
        "or `Result(user_html=..., llm_result=..., llm_images=[fig, png_bytes])`. "
        "Three dashboard panes show the session live: every running/finished run "
        "under executions, every live view (a terminal, a widget) under resources, "
        "and your curated highlight reel under cells \u2014 fill it with the most "
        "important results via `cells.add(value, title=...)` (also `.set`, "
        "`.remove`, `.clear`) so the page reads as an informative presentation, "
        "not a raw log. The dashboard URL is the `DASHBOARD_URL` variable in the "
        "namespace (share it with the human)."
    ),
)

Content = list[outputs.Content]


@mcp.tool(
    description=(
        "Run Python on the shared persistent kernel. Waits up to `budget` seconds; "
        "if the code is still running it keeps going in the background as jobs['<id>'] "
        "and this returns a job handle. Inspect/await/cancel background jobs with more "
        "python_exec against the `jobs` dict. Every run is kept there, so a reply that "
        "gets truncated is never lost: page the full run with jobs['<id>'].grep('pat') / "
        ".tail(n) / .head(n) / .slice(a, b) / .lines(a, b), or read jobs['<id>'].output "
        "(stdout) and jobs['<id>'].result (the value); history() lists recent runs. "
        "The namespace persists across calls, so "
        "functions and classes you define are reusable next turn. The kernel is one "
        "shared event loop: a blocking call (`subprocess.run`, `time.sleep`, a heavy "
        "CPU op) freezes EVERY job and your own next cell, so it MUST be wrapped in "
        "`await asyncio.to_thread(...)` (or use an async API) and backgrounded if slow. "
        "EVERY cell MUST END "
        "with a `Result(...)` or the run is rejected: `Result.text('done')` (same text "
        "to human and model), `Result.ok('what happened')` (a side-effect confirmation), "
        "`Result.of(value)` (render a DataFrame/figure/value richly for the human, its "
        "repr to you), or `Result(user_html=..., llm_result=..., llm_images=[fig])` to "
        "split the human's rich HTML view from your text+images. Curate the dashboard's "
        "presentation pane with `cells.add(value, title=...)`."
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
    # The job's captured stdout/stderr and (on failure) its traceback live in the
    # summary, not in the kernel display stream, so surface them to the caller.
    captured = summary.get("output")
    if captured:
        parts.append(outputs.text(captured))
    # Rich result blocks (images / HTML / the result repr) come from the kernel
    # display; drop the "(no output)" placeholder to_mcp emits when there were none.
    parts.extend(item for item in rendered if getattr(item, "text", None) != "(no output)")
    # When the reply was clipped to fit, the full run still lives in the kernel as
    # jobs['<id>']. Point the caller at it (with the ops to page it) so a large
    # result is recoverable without re-running the work \u2014 the failure mode this
    # whole jobs registry exists to avoid.
    job_id = summary.get("id")
    output_chars = summary.get("output_chars") or 0
    result_chars = summary.get("result_chars") or 0
    clipped = output_chars > len(captured or "") or result_chars > outputs.MAX_TEXT_CHARS
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

    hits = await _search.grep(
        pattern, top_k=top_k, case_sensitive=case_sensitive, **_scope(source, user, repo, host, project)
    )
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
    return await _gcal("cancel", event_id, "--json", "--calendar", calendar, "--notify", notify)


async def _gcal(*args: str) -> str:
    binary = os.environ.get("IX_GCAL_BIN")
    if not binary:
        raise RuntimeError("IX_GCAL_BIN is not set; the gcal binary is bundled into ix-mcp")
    proc = await asyncio.create_subprocess_exec(
        binary, *args, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE
    )
    stdout, stderr = await proc.communicate()
    if proc.returncode != 0:
        detail = stderr.decode(errors="replace").strip()
        raise RuntimeError(detail or f"gcal exited with status {proc.returncode}")
    return stdout.decode(errors="replace")
