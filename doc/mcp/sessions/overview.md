# mcp sessions and the store

Two related concerns: the SQLite execution store every run is written to
(`store.py`), and the `--session` mode that turns that one file into a reopenable
notebook whose namespace comes back (`runtime.py` checkpoint/restore +
`cli.py`/`kernel.py` wiring). The store is the single source of truth the
[runtime](../runtime/overview.md) writes and the [dashboard](../dashboard/overview.md)
reads; the session machinery layers durability and replay on top of it.

## The store (`store.py`)

One append-only SQLite file at `IX_MCP_STORE`, opened in WAL mode with a
`busy_timeout` so a reader never blocks the writer and sees in-flight rows
(`store.py:77-92`). Single-writer by contract: the kernel process owns all
writes, the dashboard and embedders only read (`store.py:1-11`). A `None` path is
rejected explicitly so it cannot silently create a file named `None`
(`store.py:85`). `_migrate` (`store.py:95`) adds columns introduced after a store
was first created, idempotently (two processes can race the `ALTER`).

Tables (`store.py:24-74`):

| table | purpose | key columns |
| --- | --- | --- |
| `executions` | every `python_exec` run | `id`, `name`, `code`, `status`, `started_at`/`ended_at`, `budget`, `output`, `result`, `error`, `line`, `error_line`, `outputs` (rich mime bundles), `bindings`, `kind` (`cell`/`replay`), `namespace` |
| `snapshots` | namespace checkpoints (only the newest kept) | `created_at`, `blob` (dill), `names`, `skipped` |
| `cells` | the agent's curated presentation reel | `id`, `title`, `position`, `outputs`, `updated_at` |
| `resources` | live self-updating views | `id`, `title`, `kind`, `html`, `status` (`live`/`closed`), timestamps |

Write API: `start` (`store.py:132`), `update_output` (a running job's live output
tail + current `line` + rich outputs, `store.py:154`), `finish` (final status,
result, error, rich outputs, bindings, namespace, `store.py:178`), `rename`,
`replace_cells` (one transaction, `store.py:290`), `upsert_resource`/
`close_resource`. Read API: `recent` (running jobs pinned first so a long run is
never dropped by the LIMIT, `store.py:230`), `get`, `latest_namespace`
(the newest finished run's globals, `store.py:243`), `cells`, `live_resources`.

## Ephemeral vs session

An ephemeral server (no `--session`) wipes its store plus the `-wal`/`-shm`
sidecars at startup so a leftover database never shows stale runs (`cli.py:373-378`).
The store path is `IX_MCP_STORE` if pinned (an embedder like the pi-harness room
mapper pins it so both sides agree on one file, `cli.py:156`), else a per-port
file in the private runtime dir (`store-<dashboard_port>.db`) so concurrent
servers never collide.

`--session FILE` (or the standalone `notebook FILE`) makes that file the durable
store, kept across restarts (`cli.py:336-368`). It refuses to combine with a
pinned `IX_MCP_STORE` rather than guess which contract was meant (`cli.py:340`).
Reopening an existing file first marks any rows left `running` by a dead server as
`interrupted` and closes live resources (`store.mark_interrupted`, `store.py:417`).
The runtime keys session behavior on `IX_MCP_SESSION=1` (`runtime.py:2103`).

## Checkpoint (`runtime.py` snapshot path)

In a session the flusher debounces a namespace checkpoint after cells finish
(`_mark_snapshot_dirty` -> `_snapshot_tick`, at most one per
`_SNAPSHOT_MIN_INTERVAL` = 5s, `runtime.py:2078`, `runtime.py:2117`). A checkpoint
serializes per-name with `dill` (`runtime.py:2131`) so functions and classes
defined in cells survive where stdlib `pickle` cannot. `_snapshot_candidates`
(`runtime.py:2186`) covers only names a user bound after the `install()` baseline;
modules, underscore names, and values no serializer can carry (open sockets, live
jobs, terminals) are skipped and reported, and a single value over
`_SNAPSHOT_MAX_VALUE_BYTES` (64 MB, `runtime.py:2122`) is skipped so the file
cannot balloon. `save_snapshot` writes the new row and prunes older ones in one
transaction, so the file holds exactly one checkpoint (`store.py:377`). The server
takes one final checkpoint on shutdown via `kernel.snapshot_session` so the last
cells' state reopens instantly (`cli.py:538-541`).

Failure is self-healing by construction: a checkpoint that fails to save leaves
the previous one in place, and replay (below) covers everything since it
(`runtime.py:2091-2093`).

## Restore (`runtime.py:2305`, `__ix_restore`)

On reopen the server runs `await __ix_restore()` while holding the shell channel,
so every tool call submitted later queues behind it and the first new cell always
sees the restored state (`cli.py:480-499`, `kernel.py:233`). `_restore_body`
(`runtime.py:2321`):

1. Load the latest checkpoint and `dill`-load each name into the namespace
   (instant state); a checkpoint that fails to decode falls back to replaying the
   full log.
2. `store.replayable(since)` (`store.py:436`) returns the original
   (`kind='cell'`) successful cells that finished AFTER the checkpoint's
   `created_at`, oldest first, and re-runs each with `kind="replay"` and a
   `_REPLAY_BUDGET` of 600s (`runtime.py:2302`, `runtime.py:2353`). Replay rows are
   excluded from future replay sets so a session never double-runs its history.
   The anchor is `ended_at` (not `started_at`) so a cell mid-run at checkpoint
   time re-runs and overwrites its partial state (`store.py:444-447`).
3. Take one fresh checkpoint folding in the replayed state, so the NEXT reopen is
   all-instant (`runtime.py:2362`).

`__ix_restore` prints a summary (names restored, not-in-checkpoint, load
failures, cells replayed, replay failures) to the server log, not a job buffer
(`runtime.py:2377`). The replaying flag (`_restoring`) blocks the debounced
checkpoint mid-restore so the anchor cannot advance past not-yet-replayed cells
(`runtime.py:2108-2113`).

## Per-MCP-session namespaces

Distinct from session FILES: the HTTP transport multiplexes several MCP clients
onto the one kernel, so each client session gets its own globals dict keyed by the
session id the server passes through `__ix_exec` (`_session_ns`,
`runtime.py:2400`). Each is seeded from the shared baseline (the helper surface)
but isolates name bindings so parallel agents do not clobber each other; the
helper OBJECTS (`jobs`/`cells`/`resources`) stay shared so the dashboard and
cross-session paging keep working. The stdio transport serves one client per
process and uses the shared namespace, which is also what session
checkpoint/restore covers (`tools.py:82-110`).
