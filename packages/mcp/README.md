# ix-mcp

A Python execution MCP server. Its one general tool, `python_exec`, runs code on
**one shared, persistent IPython kernel**. The namespace persists across calls,
many executions run **concurrently** on the kernel's event loop, and work that
outlives a short foreground budget keeps running in the background. An
auto-started dashboard shows every running execution and its live output.

## Quickstart

```
nix run .#mcp -- serve                # MCP over stdio (what an MCP client launches)
nix run .#mcp -- serve --http :8000   # MCP over streamable HTTP instead
nix run .#mcp -- dashboard            # open the running server's dashboard URL
nix run .#mcp -- eval '1 + 2'         # one-shot expression on a throwaway kernel
```

When `serve` starts it prints a dashboard URL to stderr. The dashboard is
read-only (it renders the execution log); access is gated by reachability: the
default bind is loopback, and the fleet only exposes it over Tailscale (see
[Remote access](#remote-access)).

## The main tool

`python_exec(code, budget=15, name=None)` runs `code` on the shared kernel and
waits up to `budget` seconds. If the code finishes in time you get its output and
result. If it is still running, it keeps going in the background as an entry in
the in-kernel `jobs` dict and the call returns a job handle.

Job control needs no extra tools, because the registry is just namespace state
you poke with more `python_exec`:

```python
jobs['ab12']                                # inspect: status + output tail (repr)
await jobs['ab12']                          # wait for it (use a larger budget)
jobs['ab12'].cancel()                       # stop it
[j for j in jobs.values() if j.running()]   # list running jobs
```

Anything you define on the kernel persists, so define a function or class once
and call it again on a later turn; you are building up a session, not running
one-shot snippets.

Many jobs run at once and none blocks the others. The concurrency is cooperative:
a job yields the loop at each `await`. For a blocking call (a heavy numpy/polars
op, `fff`, a subprocess) wrap it in `await asyncio.to_thread(...)` so its
GIL-releasing work runs off the loop and stays non-blocking.

## Two audiences: `Result(user_html, llm_result)`

When a cell's final value should read differently for the human watching and for
you, return a `Result`:

```python
Result(user_html="<table>…</table>", llm_result="42 rows, mean 3.1")
```

The dashboard renders `user_html` (a rich HTML view for the human) while your
`python_exec` tool result receives only `llm_result` (concise text). It is a mime
bundle under the hood (`text/html` for the dashboard, `text/plain` for you), so a
large rendered view never costs you tokens. For an ordinary value the two
audiences share one rendering: a bare trailing expression still shows its rich
repr on the dashboard and its text to you.

## How it works

`ix-mcp serve` starts one IPython kernel (over `jupyter-client`), an auto-started
read-only dashboard, and the MCP transport, all on one event loop.

- `kernel.py` owns the single kernel; `python_exec` sends `await __ix_exec(code,
  budget)` to it.
- `runtime.py` is the in-kernel runtime loaded by the IPython startup script: it
  defines `jobs`/`Job`/`__ix_exec`, runs each execution as an asyncio task,
  captures per-job stdout under interleaving with a `ContextVar`, and writes each
  run to the SQLite store.
- `store.py` is the append-only execution log (one SQLite file in WAL mode).
- `dashboard.py` serves a one-page live view of that log.
- `outputs.py` renders kernel messages for the agent (text, images).
- `tools.py` is the MCP surface: the general `python_exec`, plus `read` (pull a
  file or kernel value into the model's context while the dashboard stays quiet)
  and `kernel_trace` (an out-of-band stack dump for a wedged kernel). Everything
  else an agent needs is reachable from `python_exec` by importing the bundled
  modules.

## Pinned interpreter and bundled modules

The kernel runs on the same pinned interpreter as the server, so code can
`import` a set of bundled modules (the data libraries plus the in-house `fff` /
`view` / `tui` / `search` / `fleet` helpers) with no install step. The canonical
list lives in one place, the MCP server `instructions=` string in
[`tools.py`](./ix_notebook_mcp/tools.py); the interpreter that
backs it is assembled in [`default.nix`](./default.nix). Both are kept here
rather than re-enumerated in this README so the list cannot drift.

## Embedding (the room server)

The same rich feed the dashboard renders is the contract an embedder consumes.
`ix_notebook_mcp/feed.py` is the single source of truth for the agent's
presentation as structured data; the dashboard is one view of it, the room
server is another. Read it two ways:

- In-process: `feed.snapshot(conn)` returns `{jobs, cells, resources, rev}`, and
  `feed.job(conn, id)` returns one execution by id. `rev` is a cheap change
  marker so a poller re-renders only when something moved.
- Over HTTP (what an out-of-process embedder like the Rust room server uses):
  `GET /api/snapshot` is the whole feed; `GET /api/jobs/{id}` is one run.

`jobs` carry rich `outputs` as nbformat-style mime bundles: `text/html` is the
human view, `application/x-ix-llm+json` is what the model received. A
`python_exec` tool result already names its run as `jobs['<id>']`, so an embedder
parses that id and fetches `/api/jobs/{id}` to render that turn's tables, plots,
and HTML inline beside the agent's text. `cells` is the agent's curated highlight
reel (`cells.add(...)`), and `resources` are live, self-updating views. The JSON
shape mirrors `site/src/lib/types.ts`.

An embedder that polls the SQLite store directly (the pi-harness room event
mapper) pins its path with `IX_MCP_STORE` before launching `serve`; the server
uses that path verbatim, so both sides agree on one file. The pinning caller
owns the parent directory, and the fresh-log invariant still holds: the store
and its `-wal`/`-shm` sidecars are wiped at startup. Unset, the store is minted
in the private runtime dir, keyed by the data-API port
(`IX_MCP_DASHBOARD_PORT`, free port if unset) so concurrent servers never
collide.

## Remote access

- `IX_MCP_HOST`: the address the dashboard binds. Default is this node's
  Tailscale IPv4 (`100.x.y.z`) when Tailscale is up, else `127.0.0.1`. Set it to
  `0.0.0.0` to listen on every interface (only behind a host firewall).
- `IX_MCP_PUBLIC_HOST`: the host put into the dashboard URL.

The default Tailscale-IP bind keeps the trust boundary at the tailnet.

## Bad fit if

- You need multi-core parallelism for **pure-Python** CPU work: one kernel means
  one GIL, so pure-Python loops serialize. Offload such work to
  `asyncio.to_thread` only helps for GIL-releasing libs (numpy/polars/fff); for
  pure-Python use a subprocess / `ProcessPoolExecutor`.
- You need crash isolation between executions: they share one kernel, so a hard
  crash (a segfaulting C extension) takes the kernel down. State is recoverable
  by re-running; the durable log survives.
- You want a human and the model to read the *same* large output cheaply: the
  model reads everything it is sent, so a giant HTML blob meant for the dashboard
  costs tokens unless you split it behind a `Result(user_html=…, llm_result=…)`.
