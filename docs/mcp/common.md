# mcp

`packages/mcp` is `ix-mcp` (the `ix_notebook_mcp` Python package): a Python
execution MCP server. It exposes essentially one general tool, `python_exec`,
which runs code on a single shared, persistent IPython kernel. The kernel runs
on a pinned interpreter that already bundles the repo's developer primitives
(semantic search, file search, a PTY driver, a fleet/Ray client, browser
automation, and more) so an LLM gets those primitives by `import`-ing a module in
a cell, with no `pip`/`uv install` and no `playwright install` step. The
namespace persists across calls, executions run concurrently on one event loop,
and work that outlives a short foreground budget keeps running in the background
as a job the agent inspects with more `python_exec`. A read-only data API and a
Loro dashboard hub render every run and its live output for a human to watch.

This is one Nix package (`package.nix` declares `id = "mcp"`, so the flake output
is `.#mcp`; `nix run .#mcp -- serve`), not a Rust workspace member. It bundles
several first-party Rust cdylibs and binaries from other packages into its
interpreter (see [Glossary](#glossary)). Read this page first, then the
component page for the area you are touching.

## Components

| component | page | what |
| --- | --- | --- |
| server | [server/overview.md](server/overview.md) | the host process: `ix-mcp` CLI, transports (stdio/HTTP), the kernel manager, the MCP tools (`python_exec`/`read`/`kernel_trace`), instruction composition, the Nix build |
| runtime | [runtime/overview.md](runtime/overview.md) | the in-kernel runtime loaded into ipykernel: `jobs`/`Job`, `Result`, `cells`, `resources`, the concurrency + capture model, `api()` catalog, bundled-module registry |
| sessions | [sessions/overview.md](sessions/overview.md) | the SQLite execution store (schema, single-writer rule) and the `--session` notebook (dill checkpoint + replay-the-gap restore) |
| dashboard | [dashboard/overview.md](dashboard/overview.md) | the read-only `/api/*` data API, the `feed` embed contract, and the `pane_bridge` that publishes the MCP as a producer into the shared Loro hub |
| tool-providers | [tool-providers/overview.md](tool-providers/overview.md) | the bundled-module architecture and a table of every provider under `packages/mcp/src` (browser, fleet, nix, sh, view, ...) |
| task-graph | [task-graph/overview.md](task-graph/overview.md) | the standalone Svelte demo site and its `tasks` data generator (one schema, one generator, two consumers) |

## Units

| unit | role |
| --- | --- |
| `ix_notebook_mcp` | the server package: CLI, kernel manager, in-kernel runtime, store, dashboard data API, MCP tool surface. Nix-only Python. |
| `src/<module>` | bundled tool providers imported inside cells: `browser`, `fff`, `fleet`, `google_auth`, `imessage`, `linear`, `mcp_client`, `nix`, `nox_autotriage`, `screen`, `sh`, `slack`, `tasks`, `view`, `vmkit`, `worktree`, `x`. Each is its own Python package baked into the interpreter. |
| `task-graph/` | a Vite + Svelte demo visualizing a ~100-node dependency DAG; reads `public/tasks.sqlite` produced by the `tasks` module. |
| `default.nix` | assembles the pinned interpreter (`pkgs.python3.withPackages`), bundles every module + first-party cdylib, and wraps it as the `ix-mcp` and `ix-notebook` binaries. |

The server core is one package (`ix_notebook_mcp`) split across files: `cli.py`
(entrypoint), `kernel.py` (the one kernel + bridge), `runtime.py` (the in-kernel
runtime, the largest file), `store.py` (SQLite log), `config.py`, `transport.py`,
`tools.py` (MCP surface), `dashboard.py`/`feed.py`/`pane_bridge.py`/`produce.py`
(views), `registry.py`/`guide.py`/`requirements.py` (instructions + module
index), `outputs.py`/`introspect.py` (rendering + introspection).

## How it fits together

```
MCP client --stdio/HTTP--> ix-mcp server (kernel.py)  --execute_request-->  one ipykernel
   python_exec(code,budget)        |  one asyncio.Lock serializes the shell channel        |
                                   |                                                        v
                          tools.py / transport.py                         runtime.install(): jobs/Job/Result/
                                   |                                       cells/resources + bundled modules
                                   v                                                        |
                        SQLite store (store.py)  <---- runtime writes every run, output, namespace
                          ^               ^
              dashboard.py /api/*    pane_bridge.py -> produce.py -> Loro `dashboard` hub
              (embedders poll)       (publishes panes; human UI)
```

- One kernel, one namespace. `serve` starts exactly one ipykernel for the
  process lifetime (`kernel.py:68`). Every `python_exec` becomes
  `await __ix_exec(code, budget, ...)` sent over `jupyter-client`
  (`kernel.py:204`). A single `asyncio.Lock` serializes the shell channel
  (`kernel.py:73`, `kernel.py:135`); the per-call budget keeps each request short
  so background work never holds the channel.
- Concurrency on one loop. Inside the kernel each run is an asyncio task on the
  kernel's event loop (`runtime._runner`, `runtime.py:1189`), so many run at once
  and none blocks the others, provided no cell makes a synchronous blocking call.
  Per-job stdout is captured by a `ContextVar` (`runtime.py:49`) so interleaved
  prints land in the right job.
- The store is the single source of truth. The kernel writes every run, its
  output tail, rich outputs, and a namespace snapshot to one SQLite file
  (`store.py`); the dashboard and every embedder only read it. The feed (`jobs`,
  `cells`, `resources`) is derived from it once (`feed.py`) and exposed both
  in-process and over `/api/*`.
- Instructions and the module index are generated, not hand-written. `tools.py`
  composes the server `instructions` from `guide.py` fragments plus a
  registry-derived module/tool index (`registry.py`), so a tool or bundled module
  is declared in exactly one place and the prose cannot drift from the code.

## Invariants

- One general tool. Only `python_exec`, `read`, and `kernel_trace` are MCP tools
  (`tools.py:208`, `tools.py:289`, `tools.py:315`). Everything else (search,
  shell, calendar, git worktrees) is reached by importing a bundled module in a
  cell, so it never earns a dedicated tool.
- Budget is a hold on the one shell channel. `budget` is how long a call holds
  the kernel before the run backgrounds; it is clamped to `max_budget` (120s,
  `config.py:86`). A run that outlives its budget keeps going as `jobs['<id>']`.
- Never block the loop. A synchronous wait (`subprocess.run`, `time.sleep`,
  `requests`, a long CPU op) freezes every job; wrap it in
  `await asyncio.to_thread(...)` or use an async API (`sh`, `httpx`). A cell that
  wedges the loop past `budget + wedge_grace` (15s, `config.py:78`) is rescued by
  a SIGUSR2 watchdog (`kernel.py:216`, `runtime.py:2744`).
- stdio owns fd 0/1. In stdio mode the CLI dups the real stdin/stdout to private
  fds and points fd 0/1 at `/dev/null`+stderr before anything else writes them,
  so nothing corrupts the JSON-RPC stream (`cli.py:299`).
- Fresh log unless a session. An ephemeral server wipes its store (and `-wal`/
  `-shm`) at startup (`cli.py:377`); only `--session FILE` keeps state across runs
  and refuses to combine with a pinned `IX_MCP_STORE` (`cli.py:340`).

## Glossary

- python_exec: the one general MCP tool; runs `code` on the shared kernel, waits
  up to `budget` seconds, backgrounds the rest as a job.
- kernel: the single shared ipykernel the server drives over `jupyter-client`;
  one namespace, one event loop, for the process lifetime.
- in-kernel runtime: the code `runtime.install()` injects into the kernel
  namespace (`jobs`, `Job`, `Result`, `cells`, `resources`, `__ix_exec`, the
  bundled modules); see [runtime](runtime/overview.md).
- job: one execution, tracked in the `jobs` dict and as a row in the store; the
  agent inspects/awaits/cancels/pages it with more `python_exec`.
- Result: the opt-in split of a cell value into a human view (`user_html`) and a
  model view (`llm_result`/`llm_images`), carried as a mime bundle.
- session file (`.ixnb`): a SQLite store kept across runs; reopening restores the
  dill namespace checkpoint and replays only newer cells.
- store: the append-only SQLite execution log at `IX_MCP_STORE`; the single
  source of truth all views derive from.
- data API: the read-only aiohttp server (`/api/jobs|resources|cells|snapshot`)
  plus the gated `/api/exec` write path embedders poll.
- Loro hub / `dashboard`: the standalone Rust aggregator (the
  [dashboard](../dashboard/common.md) domain) the MCP publishes panes into as one
  producer; the human-facing UI.
- bundled module: a Python module baked into the pinned interpreter and listed in
  `registry.py`, importable in a cell with no install.
- provider: a bundled module under `packages/mcp/src` that gives the kernel a
  capability (shell, browser, fleet, ...); see
  [tool-providers](tool-providers/overview.md).
- fleet: the tailnet treated as one cluster; `import fleet` drives Ray, Spark
  Connect, SSH fan-out, and a peer's live kernel.
