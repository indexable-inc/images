"""The MCP tool surface.

Two tools only. ``python_exec`` runs code on the single shared kernel with a
foreground budget and, if the work outlives the budget, leaves it running in the
background as an entry in the in-kernel ``jobs`` dict. Job control needs no extra
tools because ``jobs`` is just namespace state: inspect/await/cancel it with more
``python_exec`` (``jobs['ab12'].cancel()``). Everything else an agent might want
(search the index, read the calendar, shell out) is reachable the same way, by
importing the bundled module inside a cell, so it does not earn a dedicated tool.

``kernel_trace`` is the one exception: it dumps the kernel's stack out of band
(a faulthandler signal, not the execute channel) so it works even when a cell has
wedged the event loop, which is exactly when ``python_exec`` cannot help.
"""

from __future__ import annotations

import json
import os
from typing import Annotated

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
        "(async file search/grep), `view`, `tui`, `exa_py`, "
        "`google_auth`, `fleet` (async polars SSH fan-out across hosts: "
        "`await fleet.read_ndjson(hosts, path)` or `await fleet.scan(hosts, cmd)`; "
        "pass `username=`/`connect_timeout=` through to asyncssh, and note that "
        "`on_error='collect'` waits out each unreachable host's full TCP timeout "
        "before dropping it, so set a short `connect_timeout` to fail fast), "
        "`nix` (run a nix build and get "
        "its internal-json as polars + a live build-DAG: "
        "`await nix.build('.#foo')` -> `.events`/`.activities` frames, `.tree()`), "
        "`sh` (when you must shell out: `out = await sh('gh run list')` runs on "
        "the loop and returns an Output that IS a Result, so the dashboard shows "
        "the command's ANSI color as HTML while you get `.text`/`.code`/`.ok` with "
        "escape codes stripped), "
        "numpy, polars, duckdb, "
        "httpx, matplotlib, playwright, "
        "htpy (build HTML in Python with `div(class_='x')[...]`; it auto-escapes "
        "every text node and attribute, so use it instead of f-string HTML, which "
        "is where escaping gets forgotten; an htpy element renders directly through "
        "`cells.add(el)`/`Result.of(el)`, so do not wrap it in `IPython.display.HTML` "
        "or stringify it). When you hand-build HTML, drive colors from CSS custom "
        "properties with a `@media (prefers-color-scheme: dark)` override (never "
        "hard-coded light-only colors), so it follows the viewer's OS theme — the "
        "dashboard is dark by default. "
        "Prefer these over shelling out: `view.ls/tree/grep/find` return "
        "polars DataFrames (composable + a styled HTML table) and "
        "`view.cat/read/head/json/diff` return a syntax-highlighted view, so "
        "you never hand-roll display HTML or run `ls`/`grep`/`cat` in bash. "
        "Prefer polars for any tabular data: return a DataFrame (or "
        "`Result.of(df)`) and the human gets the styled HTML table for free "
        "while you get the frame as compact, untruncated CSV \u2014 so you never "
        "hand-build a table and a wide/long-stringed frame is never clipped to "
        "you. Use `pl`; pandas is not bundled. "
        "To find files or code, use `fff` with polars \u2014 never shell out to "
        "`rg`/`grep`/`fd`/`find`/`ls`/`mgrep`, and never write a one-off search "
        "helper. `fff.find(query)` (typo-tolerant, frecency-ranked file find) "
        "and `fff.grep(pattern)` (SIMD content grep), plus async "
        "`fff.afind`/`fff.agrep`, return a result whose `.df` is a polars "
        "DataFrame: do the whole search by composing the polars API on it "
        "(`.filter`, `.sort`, `.group_by`, `.join`, `.head`) rather than extra "
        "functions, and it renders as the styled HTML table for free. "
        "`view.grep/find/tree/ls` return the same polars frames for a quick "
        "listing. For meaning-based recall across a corpus, `import search`. "
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
        "Write the cell as readable, idiomatic Python, not a cramped one-liner: "
        "the dashboard shows your source verbatim, so a human reads it. Use real "
        "statements over several lines, bind intermediate results to clearly named "
        "variables, and break a long comprehension or chained call across lines "
        "instead of nesting it inside a `str(...)` or a `Result(...)` call. Let the "
        "final `Result(...)` name what it returns rather than wrap one dense "
        "expression. "
        "Three dashboard panes show the session live: every running/finished run "
        "under executions, every live view (a terminal, a widget) under resources, "
        "and your curated highlight reel under cells. Treat cells as the FINAL "
        "PRESENTATION of the CURRENT state, not an append-only log: add the most "
        "important results with `cells.add(value, title=...)`, and prune stale "
        "ones as the state moves on \u2014 `cells.set(key, value)` to replace in "
        "place, `cells.remove(key)` to drop one, `cells.clear()` to start over \u2014 "
        "so the page always reflects where things stand now. The dashboard URL is "
        "the `DASHBOARD_URL` variable in the namespace (share it with the human)."
    ),
)

# Report the build's source revision as the MCP `serverInfo.version` so a client
# can see exactly which commit of the server it is talking to. The nix wrapper
# sets `IX_MCP_VERSION` to the flake rev (`<commit>` / `<commit>-dirty` / "dev");
# FastMCP does not take a version, so stamp the low-level server directly. Absent
# the env var (a bare `python -m ix_notebook_mcp`) it falls back to "dev".
mcp._mcp_server.version = os.environ.get("IX_MCP_VERSION") or "dev"

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
        "presentation pane with `cells.add(value, title=...)` — it is the final view of "
        "the current state, so prune stale cells (`cells.set`/`.remove`/`.clear`) rather "
        "than letting it grow into a log. "
        "Write readable, idiomatic Python: the dashboard shows your source verbatim, so "
        "prefer multi-line statements with named intermediates over a one-liner that "
        "nests a comprehension or chain inside a `str(...)`/`Result(...)` call."
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
    description=(
        "Dump the kernel's current Python stack for every thread. Works even when "
        "a cell has wedged the kernel by blocking its event loop with a synchronous "
        "call (subprocess.run, time.sleep, requests, a long CPU op): the dump is "
        "captured via a faulthandler signal, not the execute channel, so it returns "
        "while the loop is still frozen. Use it to see WHERE a wedged or slow cell "
        "is stuck, then fix the blocking call (wrap it in `await asyncio.to_thread"
        "(...)` and background it)."
    )
)
async def kernel_trace() -> str:
    return await current_kernel().dump_trace()
