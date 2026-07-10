# mcp dashboard and embedding

The MCP server is a data producer, not a UI owner. Three modules turn the SQLite
[store](../sessions/overview.md) into something a human or an embedder can read:
`feed.py` defines the presentation as structured data, `dashboard.py` serves it
read-only over HTTP, and `pane_bridge.py`/`produce.py` republish it as panes into
the shared Loro `dashboard` hub (the dashboard domain),
which renders the human-facing UI alongside every other producer. The store is
read-only here; the kernel owns all writes (see [runtime](../runtime/overview.md)).

## feed: the embed contract (`feed.py`)

`feed.py` is the single source of truth for the agent's presentation, consumed
both in-process (import it) and over HTTP (the `/api/*` routes wrap it). Three
kinds, mirroring the store tables and `site/src/lib/types.ts` (`feed.py:11-21`):

- `jobs`: every run, newest first with running pinned. Each carries rich `outputs`
  as nbformat-style mime bundles, where `text/html` is the human view and
  `application/x-ix-llm+json` is what the model received.
- `cells`: the agent's curated highlight reel, in display order.
- `resources`: live, self-updating views.

`snapshot(conn)` returns `{jobs, cells, resources, rev}` (`feed.py:36`); `rev`
(`feed.py:60`) is a cheap change marker built from each row's id and last-write
time plus a running job's live output length, so a poller re-renders only when
something moved without hashing whole payloads. `job(conn, id)` returns one
execution (`feed.py:53`) for an embedder to join the `jobs['<id>']` a
`python_exec` result already names. `JOBS_LIMIT` is 200 (`feed.py:33`).

## dashboard: the read-only data API (`dashboard.py`)

Auto-started by the CLI (`dashboard.start`, `dashboard.py:131`), an aiohttp app
bound to `config.host:dashboard_port`. Routes (`dashboard.py:121-127`):

| route | returns |
| --- | --- |
| `GET /` | redirect to the Loro hub URL (`dashboard.py:38`) |
| `GET /api/jobs` | `store.recent` (the `jobs` feed) |
| `GET /api/jobs/{id}` | one execution by id, 404 if absent |
| `GET /api/resources` | live resources |
| `GET /api/cells` | the presentation reel |
| `GET /api/snapshot` | the whole `feed.snapshot` (the embed contract) |
| `POST /api/exec` | the one WRITE path: run a line in THIS node's live kernel |

`/api/exec` (`dashboard.py:65`) backs a peer's `fleet.in_kernel`: it runs code in
the live kernel so a peer can read this node's real running state. It is gated two
ways (`dashboard.py:74-89`): a shared bearer token (`config.exec_token`, from
`IX_MCP_EXEC_TOKEN`(`_FILE`)) is always required if set (constant-time compared),
and trusting the bound network (`exec_trust_network`, the tailnet) is honored only
on a non-loopback bind. With neither it returns 403 (disabled, the safe default).
The body's `budget` is validated and clamped to `[0, max_budget]`.

`build_app` (`dashboard.py:30`) is split from `start` so the routes are testable
with an in-memory app and a fake kernel without binding a socket.

Bind host: the dashboard renders whatever the agent ran, so the default bind is
the node's Tailscale IPv4 when Tailscale is up (the tailnet is the trust
boundary) and loopback otherwise; `IX_MCP_HOST` overrides
(`config.py:22-26`, resolved in `cli._serve`).

## pane_bridge + produce: the Loro hub (`pane_bridge.py`, `produce.py`)

The CLI spawns the standalone `dashboard` aggregator (`IX_DASHBOARD_BIN`) as the
human UI and runs `pane_bridge.run` as a background task (`cli.py:507-508`,
`cli.py:438`). The bridge polls the store every 0.25s and republishes it as
dashboard-core panes only when the pane set changes, so an idle session is silent
(`pane_bridge.py:136`). The mapping (`pane_bridge._panes`, `pane_bridge.py:78`):
one `exec` pane per run (running -> done/failed LED, duration, the run's intent as
title), an extra `html` pane for a run's rich outputs (tables/plots/images), one
`html` pane per curated cell, one `html` pane per live resource, and one `data`
pane (the `namespace` renderer) for the kernel's live globals.

`produce.py` is the Python side of the dashboard-core producer protocol (the Rust
contract is `packages/dashboard/dashboard-core/src/pane.rs`/`publish.rs`,
`produce.py:1-15`). `PaneProducer` (`produce.py:126`) binds a unix socket in the
discovery directory and streams its full pane set as one NDJSON
`ProducerSnapshot` line to every reader (replacement semantics, so a late-joining
aggregator needs no backlog). The discovery dir is resolved in the same order as
`dashboard_core::discovery_dir`: `$IX_DASH_DIR`, then `$XDG_RUNTIME_DIR/ix-dash`,
then `/tmp/ix-dash-<user>` (`produce.py:32`). `exec_pane`/`data_pane`/`html_pane`
(`produce.py:64-123`) build the pane dicts. The whole path is best-effort: if the
hub binary is absent or the socket cannot bind, the MCP keeps working and the
data API still serves embedders, there is just no UI (`cli.py:446-454`,
`produce.py:151-165`).

## Two readers, one feed

```
store.db --(reads)--> feed.snapshot --(HTTP)--> /api/* --> embedder (room server polls /api/snapshot)
         \--(reads)--> pane_bridge --> produce (unix socket) --> dashboard hub --> human browser
```

The room server (an out-of-process embedder) spawns `ix-mcp` as its agent's only
tool and polls `/api/snapshot` to render a turn's tables/plots/HTML inline beside
the agent's text; an in-process embedder calls `feed.snapshot(conn)` directly. The
human-facing dashboard is the Loro hub, which folds the MCP's panes into one
canvas with every other producer (a TUI's terminals, a VM's screen).
