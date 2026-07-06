# mcp in-kernel runtime

`ix_notebook_mcp/runtime.py` is the code that runs INSIDE the ipykernel (not the
server process). It is loaded once per kernel by the shipped IPython startup
script (`ipython/00-ix-runtime.py:12` calls `runtime.install`) and defines the
agent-facing programming model: the `jobs` registry, the `Job` handle, `Result`,
`cells`, `resources`, the `api()` catalog, plus the `__ix_*` entrypoints the
[server](../server/overview.md) drives. It is the largest file in the package
(~2860 lines); this page is the map.

## install (`runtime.py:2763`)

`install(user_ns)` wires the runtime into the kernel: tee stdout/stderr through
`_Tee` so per-job capture works (`runtime.py:2772`), install the SIGUSR1/SIGUSR2
signal handlers (`runtime.py:2717`), capture rich display output and register
rich formatters on the shell, open the SQLite store at `IX_MCP_STORE`
(`runtime.py:2786`), and bind the runtime surface into the user namespace
(`runtime.py:2798-2848`): `jobs`, `history`, `doc`, `Job`, `Result`, `cells`,
`Cells`, `resources`, `Resource`, `register_resource`, `api`, the `__ix_*`
functions, `DASHBOARD_URL`, the pre-imported modules from
`registry.preimport_names()` (`fff`, `view`), `asyncio`/`json`/`pl`, and the
retired-but-still-bound `sh`/`zsh` disabled shims (they raise a migration hint
pointing at `await nu(...)`).
Everything bound up to this point is the runtime baseline (`_baseline_names`,
`runtime.py:2853`); only names a user binds AFTER it are covered by session
checkpoints. A second startup script, `ipython/01-ix-polars.py`, widens polars'
text repr for the agent and installs `view.df_html` as the global
`DataFrame._repr_html_` (`ipython/01-ix-polars.py:13-26`).

## Execution flow

The server sends `await __ix_exec(code, budget, name, session)`
(`runtime.py:2516`), which calls `__ix_run` then `_emit`:

- `__ix_run` (`runtime.py:2417`): pick the namespace for `session` (`_session_ns`,
  below), build a `Job`, register it in `jobs`, launch `_runner` as an asyncio
  task, and `await asyncio.wait({task}, timeout=budget)`. It returns the `Job`
  whether the run finished or is still going, so a run that outlives `budget` just
  stays in `jobs`.
- `_runner` (`runtime.py:1189`): set the per-job `ContextVar` so captured stdout
  routes to this job, write the `start` row, compile the cell (a `SyntaxError`
  becomes a failed job, not an escape), and execute it. A cell with a top-level
  `yield` is compiled as an async generator and each yielded value is displayed as
  it streams (`runtime.py:1210-1230`); otherwise the trailing expression is the
  result, coerced via `Result.of` with stdout merged in (`runtime.py:1231-1254`).
  `CancelledError` -> `cancelled`; a watchdog `KeyboardInterrupt` -> a crisp
  blocking-call message; any other exception -> `error` with a cell-trimmed
  traceback and the failing line. `finally` persists the final row and marks the
  snapshot dirty.
- `_emit` (`runtime.py:2499`) attaches the structured `_job_summary`
  (`runtime.py:2457`) as the `application/x-ix-job+json` mime bundle the server
  parses (status, running, full output/result sizes for truncation detection).

## Concurrency and capture

Each run is an asyncio task on the kernel's one event loop, so many run at once
and none blocks the others (module docstring, `runtime.py:13-18`). Per-job
stdout/stderr is captured by routing `_Tee` writes through the `_ix_current`
`ContextVar` set inside each task (`runtime.py:49`, `runtime.py:137`), so
interleaved prints from concurrent jobs land in the right job buffer (capped at
`_MAX_OUTPUT_CHARS` = 256k, `runtime.py:54`, `runtime.py:234`). The invariant
agents must respect: a synchronous blocking call freezes the whole loop; wrap it
in `await asyncio.to_thread(...)` or use an async API. A blocked cell is rescued
by the SIGUSR2 watchdog (`runtime.py:2744`, raised inline at the blocked frame
because a task-cancel never breaks a synchronous call).

## Job (`runtime.py:187`)

`Job` is the awaitable handle over the task. State: `id` (8-hex), `code`, `name`
(defaults to `id`), `kind` (`cell`/`replay`), `status`, `started`/`ended`,
`budget`, `line` (live executing line, sampled off the suspended coroutine chain
by `_current_line`, `runtime.py:1117`), `error_line`, `_result`. Public surface
agents drive with more `python_exec`:

- inspect/control: `running()`, `done()`, `ok()`, `cancel()`, `wait(timeout)`,
  `await job` (`runtime.py:325-391`); `result` raises while still running rather
  than return a misleading `None` (`runtime.py:345`).
- paging (`runtime.py:259-316`): `output` (full stdout), `tail(n)`, `head(n)`,
  `slice(a,b)`, `lines(a,b)` (numbered), `grep(pattern, ctx)`. These operate on
  `pageable` (stdout, else the result's model text), so the truncation notices in
  `tools.python_exec` reach a big returned value too.

`history(n)` (`runtime.py:2478`) lists recent runs; `doc(obj)` returns an object's
signature + docstring as a `Result` (`runtime.py:1922`).

## Result (`runtime.py:442`)

`Result` is the opt-in split of a cell value into the human view (`user_html`,
rendered on the dashboard) and the model view (`llm_result` text plus
`llm_images`), carried as a mime bundle: `text/html` for the human, the internal
`application/x-ix-llm+json` for the model, `text/plain` as a fallback
(`runtime.py:471-474`). Shortcuts: `Result.text(s)` (same text both ways),
`Result.ok(msg)` (a quiet confirmation), `Result.of(value)` (render any value
richly for the human, concise text to the model; a polars DataFrame becomes the
styled table for the human and compact untruncated CSV for the model,
`runtime.py:528`). `Result(a, b, ...)` stacks several values each with its own
view. `llm_images` accept bytes/base64/data-URI/matplotlib/PIL/path and are
downscaled and re-encoded to fit `IX_MCP_IMAGE_MAX_DIM`/`IX_MCP_IMAGE_MAX_BYTES`
(`runtime.py:1518`, `runtime.py:60-77`). A cell that never mentions `Result`
still returns exactly what a notebook would (the trailing expression plus
stdout).

## cells and resources

- `cells` (`runtime.py:826`, the `Cells` instance): the agent's curated highlight
  reel the dashboard shows as the answer. `cells.add(value, title=, id=)`,
  `set(key, value)`, `remove(key)`, `clear()`; `_sync()` (`runtime.py:932`)
  mirrors the whole ordered set into the store's `cells` table on change.
- `resources` / `Resource` / `register_resource` (`runtime.py:728-820`): live,
  self-updating views (a running terminal, a custom widget) re-rendered to HTML
  each flush tick. `_discover_tui_resources`/`_discover_vmkit_resources`
  (`runtime.py:1751`, `runtime.py:1807`) auto-publish a `tui` terminal or a
  `vmkit` guest display as a resource.

## The flusher (`runtime.py:2058`)

One throttled background loop (every 0.5s) persists each running job's output tail
and live line to the store, re-renders live resources, syncs `cells`, and (in a
session) fires a debounced namespace checkpoint. It is the single mechanism that
makes the dashboard show in-flight work without ever touching the kernel from
another process. See [dashboard](../dashboard/overview.md) and
[sessions](../sessions/overview.md).

## Discovery: `api()` and `introspect.py`

`api(filter)` (`runtime.py:1939`) is the live catalog of every helper: the
namespace builtins and each bundled module's public surface, each with its real
signature (from live introspection, never copied prose) and a one-line summary,
returned as a polars DataFrame to filter. The catalog is built from `registry.py`
(`_api_rows`, `runtime.py:1850`) so a module declared there shows up in `api()`,
gets pre-imported if marked, and is listed in the server instructions, with no
signature duplicated anywhere. `introspect.py` backs the dashboard's namespace
pane and inlay hints: `describe(value)` (`introspect.py:143`), `namespace_rows`
(`introspect.py:230`), and `cell_bindings(code, ns)` (`introspect.py:59`) snapshot
each variable's live value, type, and shape.

## The bundled-module registry (`registry.py`)

`registry.py` is the single source of truth for what the kernel offers: `MODULES`
(first-party bundled modules catalogued by `api()`, with `preimport` and optional
`credential`), `BUILTINS` (names always present: `Result`, `cells`, `jobs`,
`history`, `doc`, `resources`, `register_resource`, `api`, `asyncio`,
`json`, `pl`, `DASHBOARD_URL`), and `LIBRARIES` (bundled third-party libs:
numpy, polars, duckdb, httpx, matplotlib, pypdf, playwright, exa_py). `fff` and
`view` are pre-imported (`registry.py:62-73`). Credentialed entries declare an
external service, env vars, token path, and remedy used by `requirements.py` and
the instructions: `search` (Mixedbread), `google_auth` (Google), `slack` (Slack),
`linear` (Linear), `exa_py` (Exa). Per-provider detail is in
[tool-providers](../tool-providers/overview.md).

## Session entrypoints

`__ix_snapshot`/`__ix_restore`/`__ix_run`(`kind="replay"`) and the per-MCP-session
namespaces (`_session_ns`, `runtime.py:2400`) implement the persistent notebook;
they are documented in [sessions](../sessions/overview.md).
