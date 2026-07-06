# nix-web-monitor internals

The state machine, transport, dependency resolution, and daemon tracing behind
[overview](overview.md).

## Event parsing (`parser/src/lib.rs:1073`)

`parse_line` keys on the `@nix ` prefix (`NIX_JSON_PREFIX`, `:13`): a line
without it is `Plain`; with it, the JSON is parsed and the `action` field selects
`start`/`stop`/`result`/`msg` (`parse_event`, `:1095`). Unknown actions become
`NixEvent::Unknown` rather than an error, so a newer Nix never breaks parsing.
Result codes and activity codes are named constants (`result_code`,
`activity_code`, `:37`-`:56`).

## State machine (`MonitorState`, `parser/src/lib.rs:276`)

`apply_parsed_line` mutates an in-memory model: `activities` (by id), `builds`
(by derivation path), a capped `logs` tail, `errors`, aggregate `progress`,
`optimise` totals, `daemon` view, and `expected` counts. Two outputs ride out:

- **`snapshot()`** (`:342`): a full `MonitorSnapshot`, used once to seed a new
  client. The log tail is capped at `SNAPSHOT_LOG_LIMIT` (500, `:26`) because the
  UI only renders the tail and re-broadcast would otherwise be O(n^2) in build
  verbosity. Server-side retention is capped separately (`STATE_LOG_RETAIN`
  5000, `STATE_ERROR_RETAIN` 2000) from the head.
- **`drain_deltas()`** (`:367`): the incremental `Delta`s accumulated since the
  last drain, broadcast per applied line. `Delta` (`:248`) is a tagged union
  (`BuildUpsert`, `ActivityUpsert`, `LogsAppend`, `ProgressSet`, `OptimiseSet`,
  `DaemonSet`, `ExpectedSet`, `ErrorAppend`, `DependenciesSet`, `Finished`, plus
  `Reset` used only for the seed).

Non-obvious behaviors:

- **No positive success marker.** Nix never says an activity succeeded, so
  `finish(exit_code)` (`:475`) waits for the wrapped process: on a clean exit it
  promotes still-`Stopped`/`Planned` builds to `Succeeded`. `BuildStatus`
  (`:1042`) is `Planned` (announced in the build plan, seeds the tree top-down) ->
  `Running` -> `Stopped` -> `Succeeded`/`Failed`.
- **CA resolve folding.** A content-addressed build emits `resolved derivation:
  A -> B` then builds `B`; `resolved_to_original` (`:311`) maps `B` back to `A` so
  the row stays the one the user asked for, flagged `content_addressed`
  (`:1037`), instead of a look-alike pair.
- **Build plan sections.** `PlanSection` (`:63`) tracks whether the indented
  store paths after a "these N derivations will be built" header belong to the
  build list (seeded as `Planned`) versus the "will be fetched" list.
- **Optimise totals.** Per-file `FileLinked` results are summed into
  `OptimiseStats` (`:229`) so the run-wide hard-linking cost behind a slow
  "copying to the store" is visible; Nix reports no aggregate.

## Transport (`server/src/main.rs`)

`run_nix_command` (`:325`) spawns `nix --log-format internal-json` (and `-v`
under `--nix-verbose`), forwards stdout byte-for-byte (so `nix eval --raw` is
exact, `:380`), and reads stderr line by line, decoding lossily so a builder's
non-UTF-8 byte never stalls the pipe (`parse_stderr`, `:405`). Each line is
applied under the write lock, then `broadcast_deltas` (`:555`) drains and sends
the deltas as msgpack binary frames; draining and sending under one lock keeps
concurrent callers from interleaving frames out of order.

Per-client (`serve_socket`, `:233`): seed with a `Reset` snapshot and subscribe
under the read lock (no gap or duplicate between seed and stream), then forward
broadcast frames. A client that outruns the `DELTA_CHANNEL_CAPACITY` (1024) ring
is re-seeded with a fresh snapshot rather than replayed (replay would
double-apply non-idempotent `LogsAppend` deltas, `:281`). Sends are bounded by
`SEND_TIMEOUT` (30s) so a stalled reader is dropped, not pinned (`:310`).

## Dependency DAG (`server/src/dependencies.rs`)

The internal-json stream has no edges, so for every derivation Nix reports,
`resolve_dependencies` (`:38`) queries `nix-store --query --requisites` (the
decade-stable interface, not the version-dependent `nix derivation show` JSON,
`:94`) for its full transitive `.drv` closure and feeds it to
`MonitorState::record_closure`. Querying the transitive closure (not just direct
refs) lets the DAG bridge through cached intermediates Nix never reports building
(`parser/src/lib.rs:420`). `snapshot` reduces the closures to a minimal
`DerivationEdge` set over built derivations only (`:355`), and `emit_dependencies`
(`:437`) suppresses an unchanged edge set so redundant `DependenciesSet` frames
do not ride the wire. Queries run on their own task, bounded to
`MAX_CONCURRENT_QUERIES` (16, `:22`) so a large build's hundreds of derivations
fill the tree while builds are still in flight.

## Copy-size measurement (`server/src/main.rs:482`)

Nix reports a local "copying <path> to the store" as an unstructured activity
with no byte progress. `copy_to_store_source` (`parser/src/lib.rs:941`) extracts
the source path; the server then walks it on a blocking thread following
gitignore semantics (skipping `.git`, `parents(false)`), sums apparent file
sizes, and attaches the figure with `set_activity_size` (`:396`). It is an
approximate hint: a failed walk leaves the row unannotated rather than failing
the build (`copied_size`, `:516`).

## Daemon syscall tracer (`server/src/daemon.rs`)

`run_daemon_probe` (`:43`) is the one view the internal-json stream cannot give:
it attaches a platform tracer to the running `nix-daemon` so the silent
`addToStore` phase is visible. It finds daemon pids via `pgrep` (`:72`), then
spawns `fs_usage -w -f filesys nix-daemon` on macOS or `strace -f -p <pid>` on
Linux (`tracer_command`, `:101`), wrapped in `sudo -n` when not root (`-n` never
prompts, so a user without privilege gets a "needs root" status instead of a
hang, `:98`). The tracer's stdout lines are parsed by the parser's
`parse_fs_usage_line`/`parse_strace_line`, folded into a `DaemonTrace`, and a
`DaemonInfo` is published every `SAMPLE_INTERVAL` (exactly 1s, so the per-window
syscall delta is the per-second rate with no division, `:30`). Path-bearing
syscalls also update a one-second hot-path window; the panel lists the busiest
paths by current rate, then cumulative count, so a silent build has a concrete
\"what is doing the most\" readout instead of only the latest touched path. Every
failure path (no daemon, missing tracer, denied attach) degrades to a status
string the panel shows and the loop retries after `RETRY_INTERVAL` (5s); the
probe never returns an error (`:41`). `OpClass::classify`
(`parser/src/daemon.rs:37`) groups syscalls so the panel shows work kind
(Link/Rename dominate store optimisation, Write/Fsync dominate writing a path).

## Machine-wide build view (`server/src/global.rs`)

Everything above watches *one* nix invocation; `run_global_probe` watches the
whole machine. It polls the patched-nix `nix store builds --json` subcommand
(the `build-status-dir` experimental feature: every active build/substitution
goal writes a status file under `<nixStateDir>/status/`), whose entries carry
the derivation, worker pid, start time, requesting client user/uid, on-disk log
path, and a why-chain walked up the goal's waiters to the root goal the client
asked for. Detection is by result: if no invocation variant parses as a JSON
build array (stock nix prints an "unknown command" error), the view is marked
undetected and the panel hides; the probe re-probes every 30s so a mid-session
nix upgrade is picked up. The parser side (`parser/src/global.rs`) owns the
tolerant wire types: unknown fields are ignored, missing optionals become
`None`, and an unknown goal kind folds to `Other`, so C++-side schema drift
degrades one field rather than the probe.

The panel groups rows by requesting user and renders each goal's why-chain as a
provenance trail ("for <root> via <hop> → <hop>"), so any leaf build is
attributable to the top-level thing that wanted it. Each build row can expand a
live log tail served by `/api/global-log?drv=<drvPath>`: the server resolves the
drv against the *current* machine view (never a caller-supplied path), then
reads and bzip2-decompresses the log itself. Nix compresses build logs while
writing them, so a live log is a truncated stream `nix log` refuses;
`decompress_prefix` keeps everything decoded before the truncation point and
`tail_lines` bounds the response to the newest 64 KiB at a line boundary.
