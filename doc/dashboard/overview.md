# dashboard

`packages/dashboard` is the standalone aggregator: one web canvas for every
resource producer on the machine. It scans the discovery directory, connects to
every producer socket, folds each producer's stream into one Loro document under
its own scope, and serves the shared board over HTTP + SSE. No producer owns the
server and exactly one process binds the TCP port, so any number of producers
can come and go behind one stable URL.

It is a thin binary over [dashboard-core](../dashboard-core/overview.md): it owns
no wire types, no transport, and no rendering, only the process wiring (CLI,
runtime, signal handling) plus a self-contained `demo` producer. Source is one
file, `packages/dashboard/dashboard/src/main.rs`.

## Build and run

Rust workspace binary (`packages/dashboard/dashboard/Cargo.toml`: `[[bin]] name =
"dashboard"`), built as the `dashboard` flake output
(`packages/dashboard/dashboard/package.nix`: `flake = true`, so `meta.mainProgram =
"dashboard"`). Dependencies: `dashboard-core`, `clap`, `serde_json`, `tokio`
(`Cargo.toml:15`).

```
nix run .#dashboard                      # serve on 127.0.0.1:8080, watch the discovery dir
nix run .#dashboard -- --port 0          # ephemeral port, printed on startup
nix run .#dashboard -- demo              # self-contained producer, no other process needed
nix build .#dashboard                    # build the binary
```

`packages/dashboard/dashboard/default.nix` selects the workspace binary via
`ix.cargoUnit.selectBinaryWithTests` and additionally exposes the nix-built
Svelte site under `passthru.tests.site` as a build check. The page itself is
embedded into `dashboard-core` at compile time (see
[dashboard-core](../dashboard-core/overview.md#the-embedded-page-srcdashboardserverrs-buildrs));
this binary carries no runtime asset dependency.

## CLI (`src/main.rs:29`)

| flag | default | meaning |
| --- | --- | --- |
| `--host` | `127.0.0.1` | bind address; `0.0.0.0` exposes it on the network (`main.rs:33`). |
| `--port` | `8080` | bind port; `0` picks an ephemeral port, printed on startup (`main.rs:37`). |
| `--dir` | discovery dir | producer-socket directory to watch (serve) or publish into (demo); global so it works before or after the subcommand (`main.rs:44`). Defaults via [`discovery_dir`](../dashboard-core/overview.md#discovery-paths). |
| `--rescan-ms` | `500` | how often to rescan the directory for new/removed sockets (`main.rs:49`). |
| `--record-ms` | `5000` | how often to persist the board as a replayable recording; `0` disables on-disk recording (replay still works live in the browser) (`main.rs:55`). |
| `--record-dir` | recordings dir | where recordings are written (`main.rs:61`). Defaults via the `RecordingStore` resolver. |

Subcommand `demo` (`main.rs:68`): publish one pane of every kind to the
discovery directory until interrupted.

## Serve flow (`run_server`, `src/main.rs:85`)

1. Resolve `dir` (`--dir` or [`discovery_dir`]) and parse the bind `SocketAddr`.
2. Create a [`Hub`](../dashboard-core/overview.md) and take the current tokio
   runtime handle (the process runtime outlives the dashboard, so the server and
   discovery loop run for the binary's lifetime, `main.rs:103`).
3. Open the [`RecordingStore`](../dashboard-core/internals.md#recordings-srcdashboardrecordingsrs)
   (`--record-dir` or default). A store failure is logged and recording is
   disabled, not fatal (`main.rs:95`).
4. [`serve_hub`](../dashboard-core/overview.md#web-surface-serve_hub-srcdashboardserverrs)
   binds the listener and starts the server; the binary prints the URL and the
   watched directory (`main.rs:105`). The returned shutdown receiver is held for
   the binary's lifetime.
5. If `--record-ms > 0`, `store.spawn_recorder(hub, interval, handle)` starts the
   periodic recorder and its task is attached with `Dashboard::push_task`
   (`main.rs:118`).
6. [`subscribe`](../dashboard-core/overview.md#consumer-side-subscribe-srcsubscribers)
   yields a `ProducerEvent` stream; a spawned loop folds each event into the hub:
   `Snapshot` -> `hub.apply_scope(producer, panes)`, `Gone` ->
   `hub.remove_scope(producer)` (`main.rs:138`). The transport (directory scan,
   per-socket read, stale-socket reaping) lives in `dashboard-core::subscribe`,
   shared with [ix-windows](../ix-windows/overview.md).
7. On `Ctrl-C`, `dashboard.stop()` aborts the server and tasks, then a final
   snapshot is written so the recording does not lose the last interval of
   changes the aborted recorder task would otherwise drop (`main.rs:150-158`).

Scope discipline: the aggregator passes the wire `producer` id straight through
as the hub scope, so each producer's panes stay isolated (see the scope
invariant in [dashboard-core internals](../dashboard-core/internals.md#reconcile-apply_scope--remove_scope)).

## Demo producer (`run_demo`, `src/main.rs:174`)

`dashboard demo` binds a [`Publisher`](../dashboard-core/overview.md#producer-side-publish-srcpublishrs)
at [`socket_path`] (or `<dir>/<pid>-demo.sock`) and republishes `demo_panes(tick)`
once a second (`main.rs:185`). `demo_panes` (`main.rs:201`) emits one of each
kind to exercise the whole pipeline and every renderer:

- a `terminal` pane with a green SGR `tick` line and a growing bar,
- an `html` pane (a producer-rendered fragment),
- an `exec` pane alternating running/finished, the finished state carrying an
  inline `trace` so the inline-trace view is exercised,
- a `data` pane with the `kv` renderer and nested JSON.

Run `dashboard demo` in one shell and `dashboard` in another to see the board
populate with no MCP or `tui` process running.

## Producers in the wild

The aggregator is producer-agnostic: anything that drives a `Publisher` appears.
Today's producers (other domains) are the `tui` crate (its PTY manager adapted
into terminal panes) and the MCP's `pane_bridge.py` (one `exec` pane per run,
one `html` pane per live resource, one `data`/`namespace` pane for the kernel
globals). The `dashboard demo` subcommand is the self-contained one.
