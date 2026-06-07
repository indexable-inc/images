"""Composable guidance fragments for the MCP surface.

The server instructions AND every tool description are assembled from the named
fragments below rather than spelled out in full at each site, so a rule -- how a
blocking call wedges the kernel, what a cell must return, how to page a job -- is
authored once here and reused wherever it applies. `compose(*parts)` joins
fragments into one normalized block; `tools.py` wires them together (and derives
the per-tool overview from the registry, so nothing restates a tool by hand).
"""

from __future__ import annotations


def compose(*parts: str) -> str:
    """Join guidance fragments into a single space-separated block, collapsing each
    fragment's incidental whitespace so callers can pick any subset in any order."""
    return " ".join(" ".join(part.split()) for part in parts if part)


# --- shared rules: reused by the kernel guide AND by tool descriptions ---

NAMESPACE = (
    "The namespace persists across calls, so variables, functions, classes, and imports you "
    "define stay defined and are reusable by every later call — define a helper once and call it "
    "again next turn."
)

JOBS = (
    "Each call runs as an async task and waits up to `budget` seconds; if the work is still going "
    "it keeps running in the background and the call returns a job handle. Background jobs live "
    "in the `jobs` dict, so manage them with more python_exec: `jobs['ab12']` to inspect, `await "
    "jobs['ab12']` to wait, `jobs['ab12'].cancel()` to stop, `[j for j in jobs.values() if "
    "j.running()]` to list."
)

PAGING = (
    "Every run is kept in `jobs`, so a truncated reply is never lost: page the run with "
    "jobs['<id>'].tail(n) / .head(n) / .slice(a, b) / .grep('pat') / .lines(a, b), or read "
    "jobs['<id>'].output (stdout) and jobs['<id>'].result (the value); history() lists recent "
    "runs."
)

BLOCKING = (
    "Many jobs run at once and none blocks the others, but only if you never block the one shared "
    "event loop. Any synchronous wait (`subprocess.run`, `time.sleep`, `requests`, a long "
    "CPU-bound numpy op) freezes the WHOLE kernel: every other job and even your own next "
    "status-check cell stall behind it until it returns. So a blocking call MUST be made "
    "non-blocking: wrap it in `await asyncio.to_thread(...)`, or prefer the async API (`fff`, "
    "`httpx`, `asyncio.create_subprocess_exec`), and run anything slow as a background job you "
    "poll, never inline."
)

RESULT_CONTRACT = (
    "Return results through `Result`, never `print`: a cell's stdout is NOT sent to you and is "
    "hidden in the dashboard by default (it is kept only for paging via jobs['<id>'].output), so "
    "surface anything worth seeing as a Result. A cell must either END with a `Result(...)` or "
    "`yield Result(...)` one or more times — each yielded Result streams to both the human and "
    "you the moment it is produced, so prefer yielding as you go to report progress and partial "
    "results. The kernel rejects a cell that neither ends with nor yields a Result."
)

# --- kernel guide only ---

INTRO = (
    "Run Python on one shared, persistent kernel with `python_exec`."
)

DISCOVER = (
    "Call `api()` (always in the namespace, no import) to list every helper you have — the kernel "
    "builtins (`Result`, `cells`, `jobs`, `sh`, plus `fff` and `view`, all bound with no import) "
    "and each bundled module's functions with "
    "signatures and summaries — or `api('grep')` to filter; reach for it instead of guessing "
    "names or grepping source."
)

VERIFY = (
    "Verify a change by its actual effect, not by a proxy: when you change "
    "something whose result a static check cannot see — an interactive UI, a "
    "rendered page, a runtime behaviour — exercise it and observe the outcome "
    "(screenshot it with the bundled `playwright`, run the path, diff the live "
    "state) BEFORE reporting it done. A green type-check or linter is necessary "
    "but not sufficient: 'it compiles' is not 'it works', and 'the tab switches "
    "in the source' is not 'the tab switches on screen'."
)

MODULES = (
    "Bundled modules need no install step. `fff` (async file search/grep) and `view` are already "
    "bound in the namespace, so use them with no import (an explicit `import fff` returns the same "
    "object); the rest you `import` once and reuse: `tui`, "
    "`exa_py`, `google_auth`, `fleet` (async polars SSH fan-out across hosts: `await "
    "fleet.read_ndjson(hosts, path)` or `await fleet.scan(hosts, cmd)`; pass "
    "`username=`/`connect_timeout=` through to asyncssh, and note that `on_error='collect'` waits "
    "out each unreachable host's full TCP timeout before dropping it, so set a short "
    "`connect_timeout` to fail fast), `nix` (run a nix build and get its internal-json as polars "
    "+ a live build-DAG: `await nix.build('.#foo')` -> `.events`/`.activities` frames, `.tree()`; "
    "and `await nix.attrs()` catalogs the flake's buildable attrs as a polars frame, so you stop "
    "guessing attr paths), `sh` (when you must shell out: `out = await sh('gh run list')` runs on "
    "the loop and returns an Output that IS a Result, so the dashboard shows the command's ANSI "
    "color as HTML while you get `.text`/`.code`/`.ok` with escape codes stripped), numpy, "
    "polars, duckdb, httpx, matplotlib, playwright,"
)

HTML = (
    "htpy (build HTML in Python with `div(class_='x')[...]`; it auto-escapes every text node and "
    "attribute, so use it instead of f-string HTML, which is where escaping gets forgotten; an "
    "htpy element renders directly through `cells.add(el)`/`Result.of(el)`, so do not wrap it in "
    "`IPython.display.HTML` or stringify it). When you hand-build HTML, drive colors from CSS "
    "custom properties with a `@media (prefers-color-scheme: dark)` override (never hard-coded "
    "light-only colors), so it follows the viewer's OS theme — the dashboard is dark by default."
)

VIEW = (
    "Prefer these over shelling out, and never reach for a subprocess to do them: `view.ls(path)` "
    "/ `view.tree(path)` list a directory as a polars DataFrame you can `.filter` / `.sort` "
    "(`view.tree` prunes noise — anything the repo's `.gitignore` ignores, plus a denylist of heavy "
    "dirs like node_modules / target / dist — so a project's structure is not buried under vendored "
    "or generated files; an ignored dir collapses to one row, an ignored file drops, and `all=True` "
    "shows everything) "
    "(never `ls` — not via bash, `sh`, or `asyncio.create_subprocess_exec`, and never paste a raw "
    "`ls -la` dump at the human), `view.grep` / `view.find` search as DataFrames, "
    "`view.cat/read/head/json/diff` return a syntax-highlighted view, and `view.edit(path, old, "
    "new)` applies a change and returns it as a highlighted diff (never edit blind). Shelling out "
    "to `ls` / `cat` / `grep` / `find` is always wrong here — you throw away the table and the "
    "highlighting and dump unstyled text the human has to read."
)

POLARS = (
    "Prefer polars for any tabular data: return a DataFrame (or `Result.of(df)`) and the human "
    "gets the styled HTML table for free while you get the frame as compact, untruncated CSV — so "
    "you never hand-build a table and a wide/long-stringed frame is never clipped to you. Use "
    "`pl`; pandas is not bundled. Even key/value data — environment variables, a config dict, "
    "counts — is tabular: return a two-column DataFrame, never a `\\n`-joined string or a printed "
    "dict."
)

SEARCH = (
    "To find files or code, use `fff` with polars — never shell out to "
    "`rg`/`grep`/`fd`/`find`/`ls`/`mgrep`, and never write a one-off search helper. "
    "`fff.find(query)` (typo-tolerant, frecency-ranked file find) and `fff.grep(pattern)` (SIMD "
    "content grep), plus async `fff.afind`/`fff.agrep`, return a result whose `.df` is a polars "
    "DataFrame: do the whole search by composing the polars API on it (`.filter`, `.sort`, "
    "`.group_by`, `.join`, `.head`) rather than extra functions, and it renders as the styled "
    "HTML table for free. `view.grep/find/tree/ls` return the same polars frames for a quick "
    "listing. `fff.map(pattern)` groups hits into a foldable code map with definitions ranked "
    "first, for “where is X defined and used?”. For meaning-based recall across a corpus, `import "
    "search`."
)

RESULT_SPLIT = (
    "A Result splits the human view from your view: the dashboard renders `user_html` for the "
    "human while your tool result gets `llm_result` text plus any `llm_images`, so a big rendered "
    "view never costs you tokens and you can hand yourself back images."
)

RESULT_VARIANTS = (
    "Use the shortcuts: `Result.text('done')` (same text to both), `Result.ok('what happened')` "
    "(a quiet confirmation for a side-effecting cell), `Result.of(df)` (render any value richly "
    "for the human, its repr to you), or `Result(user_html=..., llm_result=..., llm_images=[fig, "
    "png_bytes])`."
)

READABLE = (
    "Write the cell as readable, idiomatic Python, not a cramped one-liner: the dashboard shows "
    "your source verbatim, so a human reads it. Use real statements over several lines, bind "
    "intermediate results to clearly named variables, and break a long comprehension or chained "
    "call across lines instead of nesting it inside a `str(...)` or a `Result(...)` call. Let the "
    "final `Result(...)` name what it returns rather than wrap one dense expression."
)

CELLS = (
    "Three dashboard panes show the session live: every running/finished run under executions, "
    "every live view (a terminal, a widget) under resources, and your curated highlight reel "
    "under cells; its address is the `DASHBOARD_URL` value in the namespace (read the variable — "
    "there is no `dashboard()` function to call). Answer THROUGH cells by default: the cells pane "
    "is what the human reads as the answer, so put any result worth seeing there with "
    "`cells.add(value, title=...)` (a DataFrame, a figure, a `view`/`fff` render, an htpy "
    "element) rather than leaving it only in your tool text. Treat cells as the FINAL "
    "PRESENTATION of the CURRENT state, not an append-only log: add the most important results "
    "with `cells.add(value, id=..., title=...)` (a stable `id=` makes a re-run replace that cell "
    "in place instead of stacking a duplicate), and prune stale ones as the state moves on — "
    "`cells.set(key, value)` to replace in place, `cells.remove(key)` to drop one, "
    "`cells.clear()` to start over — so the page always reflects where things stand now."
)


# --- tool descriptions (composed into @mcp.tool(description=...)) ---

PYEXEC_INTRO = (
    "Run Python on the shared persistent kernel. Waits up to `budget` seconds; if the code is "
    "still running it keeps going in the background as jobs['<id>'] and this returns a job handle "
    "(inspect / await / cancel it via more python_exec on the `jobs` dict)."
)

SEE_INSTRUCTIONS = (
    "The server instructions cover the rest — the bundled tooling (fff / view / nix / fleet / "
    "polars / htpy), how to find and read things, and how to curate the dashboard's cells."
)

READ = (
    "Read a file (or a kernel value) into YOUR context WITHOUT spamming the human's dashboard: "
    "the full text comes back to you, while the dashboard shows only a one-line note (path, line "
    "span, size). Use this instead of `cat` / `view.cat` or printing a big value through "
    "python_exec whenever the content is for you to read, not for the human to look at — a normal "
    "cell's result streams to BOTH audiences, so it would flood the dashboard. `target` is read "
    "as a file when it names an existing file, otherwise it is evaluated as a Python expression "
    "in the kernel namespace (e.g. `jobs['ab12'].output` to page a job, or a variable you bound "
    "earlier). Pass `start` / `end` for a 1-based inclusive line range."
)

TRACE = (
    "Dump the kernel's current Python stack for every thread. Works even when a cell has wedged "
    "the kernel by blocking its event loop with a synchronous call (subprocess.run, time.sleep, "
    "requests, a long CPU op): the dump is captured via a faulthandler signal, not the execute "
    "channel, so it returns while the loop is still frozen. Use it to see WHERE a wedged or slow "
    "cell is stuck, then fix the blocking call (wrap it in `await asyncio.to_thread(...)` and "
    "background it)."
)


# --- appended once the dashboard has bound a port ---

_DASHBOARD_URL_NOTE = (
    "This session's live dashboard (every running job, its output, and your curated cells) is at "
    "{url} -- share it with the human now. It is also the `DASHBOARD_URL` variable in the kernel "
    "namespace."
)


def dashboard_note(url: str) -> str:
    """The live-dashboard sentence the CLI folds into the instructions once the
    dashboard has a URL (see tools.set_dashboard_url)."""
    return _DASHBOARD_URL_NOTE.format(url=url)
