# Engine: the IR run graph and DAG runtime

`elixir/lib/symphony_elixir/ir/` and `runtime/` are the deterministic core:
the durable run-graph model the [DSL](../dsl/overview.md) interpreter emits into,
and the supervised per-run scheduler that walks it. One `Runtime` GenServer owns
one active run; it schedules ready nodes as monitored BEAM tasks, commits each
result into the durable `RunGraph`, and resolves the run when every node is
terminal (`runtime.ex:1-10`). The actual agent turn is run by a Rust room-server
the runtime reaches only through the [engine contract](contract.md).

## IR data model (`ir/`)

- **`IR.Node`** (`ir/node.ex`) - one durable unit the runtime schedules,
  persists, recovers, and shows operators. Kinds: `:agent` (an engine turn,
  carries an `envelope` and prompt), `:exec` (a pack shell script), `:subrun` (a
  child run), and the `:gate`/`:map_fanout` dynamic-expansion placeholders
  (`ir/node.ex:30-37`). States: `pending ready running succeeded failed skipped
  upstream_failed retrying cancelled stranded` (`ir/node.ex:67-77`). `id` is
  content-derived (hash of `ast_origin` + `expansion_key`) so a logical node keeps
  its id across a deterministic replay. `deps` is DERIVED from `inputs` via
  `deps_from_inputs/1`, never hand-written (`ir/node.ex:23-28`, `170-180`).
- **`IR.Attempt`** (`ir/attempt.ex`) - one execution attempt of a node, with the
  executor (`:codex`/`:claude`/`:pi`/`:exec`/`:subrun`), `thread_id`, state,
  outcome, and cost. A node accumulates attempts across retries and recoveries.
- **`IR.RunGraph`** (`ir/run_graph.ex`) - the durable per-run state: the reified
  `ast`, the materialized `nodes`, the `trigger` context, the `placement`, an
  append-only `expansion_log`, and an `audit_log` of operator actions. Statuses:
  `pending running succeeded failed cancelled`. `source_hash` snapshots the
  `.sym` bytes the run started with, so editing the pack never perturbs runs in
  flight (`ir/run_graph.ex:21-23`).
- **`IR.Graph`** (`ir/graph.ex`) - the pure graph algebra (no process state, no
  IO): `ready_nodes/1` (schedulable kinds with every dep `:succeeded`, sorted by
  id for deterministic order), `apply_output/3` (commit a result and re-derive
  dependents), `propagate_upstream_failed/2`, `reset_node/2`, and
  `finished_status/1`. Placeholder kinds are excluded from scheduling
  (`ir/graph.ex:39-54`). Failure propagates: a failed node marks each still-waiting
  transitive dependent `:upstream_failed` unless it opts to run on failure
  (`inputs["__on_failure__"] == {:literal, true}`, `ir/graph.ex:137-150`).

## Materialization (`ir/materializer.ex`)

The seam between the interpreter and the durable graph. `materialize/3` builds the
initial `RunGraph`, validating every agent envelope at load (`from_map/1`) and
failing the whole run with `{:invalid_envelope, node_id, reason}` rather than
scheduling a malformed node (`ir/materializer.ex:35-61`). `expand_dynamic/1`
re-expands the AST against the outputs of succeeded nodes and merges by id
(`ir/materializer.ex:63-114`):

- a never-seen id is added;
- an existing `:pending` node is replaced (this is how a deferred prompt folds
  from `{:inline, nil}` to its real text once the awaited output arrives),
  preserving `created_at`/`attempts` and unioning deps;
- an existing `:running` or terminal node keeps its live state, never clobbered.

A resolved gate's placeholder is retired to `:skipped` so it leaves the
schedulable set and never deadlocks the run (`ir/materializer.ex:121-139`). This is
exactly the restart-replay invariant: a live re-expansion and a cold replay
produce identical graphs.

## Durable store (`ir/store.ex`)

One JSON file per run under `runs_dir/ir/<run_id>.json`, written atomically
(temp-then-rename) so a crash mid-write never leaves a half file
(`ir/store.ex:1-13`). `Runtime` calls `persist/2` after every transition.
`load_all/1` quarantines a file that fails to decode as `<run_id>.json.bad` and
keeps booting, so one corrupt run never blocks startup (`ir/store.ex:42-65`).
Tuples that JSON cannot represent (`{:node, id, path}`, AST fragments) round-trip
through `:erlang.term_to_binary/1` + Base64, decoded with
`:erlang.binary_to_term/1` (not `:safe`, because the store is root-owned local
state and `:safe` would refuse to recreate symphony's own failure-path atoms,
`ir/store.ex:228-234`, `390-405`). Enum strings decode against each owning
module's set, so a tampered file cannot mint arbitrary atoms (`ir/store.ex:371-388`).

## The runtime GenServer (`runtime.ex`)

One `Runtime` per run, supervised by `Runtime.Supervisor` (a `DynamicSupervisor`,
one child per run, `runtime/supervisor.ex:1-18`). The scheduling loop
(`advance`/`schedule`, `runtime.ex:336-447`):

1. If every node is terminal, `finish/1` stamps the final status and stops a
   succeeded/cancelled run (a failed run stays alive so operators can act on it,
   `runtime.ex:352-366`).
2. Otherwise it schedules `Graph.ready_nodes/1`: each is marked + persisted
   `:running`, its placement acquired if needed, then run in a
   `Task.Supervisor.start_child` task whose pid the runtime monitors itself
   (`runtime.ex:412-447`).
3. A task reports `{:node_done, id, result, thread_id}`; the runtime records the
   attempt, applies the output, and on success re-expands the AST to emit any
   newly-justified gate/fan-out children before the next pass (`runtime.ex:258-271`).

Each kind is dispatched to its executor (`runtime.ex:802-809`): `:agent` to the
injected `EngineClient`, `:exec` to `ExecRunner`, `:subrun` to `SubrunRunner`. The
engine client is injected (`Runtime.RoomEngineClient` in production), so tests
drive the runtime with an in-process fake and no room-server
(`runtime/supervisor.ex:16-17`).

### Crash recovery and the deadlock guard

Two failure modes, same conservative bias (`runtime.ex:12-38`):

- **Executor crash.** A monitored task's `:DOWN` that arrives without a prior
  `{:node_done, ...}` means the task died mid-attempt. The runtime cannot assume
  the turn had no side effect, so it marks the attempt `:stranded` and routes by
  the non-idempotent retry policy in [`Runtime.Recovery`](#recovery) (auto-retry
  only when the node opted in and showed no side effect, else `:stranded`).
- **BEAM restart.** At boot `Runtime.Supervisor.resume_pending/1` reloads every
  non-terminal run from `IR.Store` and restarts it with `recover: true`
  (`runtime/supervisor.ex:49-69`); the runtime reconciles orphaned `:running`
  nodes through `Recovery.reconcile/2` before its first scheduling pass.
- **Deadlock guard.** A pass with no ready nodes, no live tasks, and a
  non-terminal run cannot progress; the runtime fails it with `:deadlock` rather
  than hanging (`runtime.ex:373-398`). A graph that materialized to zero
  schedulable work (every gate resolved its body off) is a no-op completion, not a
  deadlock.

### Operator hooks

`cancel/2`, `retry_node/3`, `rerun/2`, and `clear_failed/2` are the operator
surface (`runtime.ex:103-146`), each recorded in the `RunGraph.audit_log`. They
back the `POST /api/v1/ir/runs/:run_id/*` routes; a run with no live process
returns 409 (see [operations](../operations/overview.md#dashboard-and-json-api)).
`clear_failed` resets `:failed`/`:upstream_failed`/`:stranded` nodes to `:pending`
and reschedules, leaving succeeded nodes intact, the surgical recovery after
fixing a failure cause.

## Recovery

`runtime/recovery.ex` is the correctness core of restart. `replay/2` re-runs each expansion in log order to
rebuild the materialized graph; the asserted invariant is `replay(ast, log) ==
live graph` (`runtime/recovery.ex:10-16`). `reconcile/2` resolves each orphaned
`:running` node by probing `EngineClient.status/1`: `:running` is left alone (the
engine still owns it), `{:finished, result}` is harvested, `:unknown` falls to the
strand/retry policy (`runtime/recovery.ex:88-105`). `auto_retryable?/1` is the
locked safety rule: opt-in (`inputs["__retry__"] == {:literal, true}`), no
observed side effect (no `thread_id` recorded), and under the 3-attempt budget
(`runtime/recovery.ex:126-139`). The synchronous room-server path has no
probe-by-thread route, so the production `status/1` returns `:unknown`
conservatively (`runtime/room_engine_client.ex:29-37`).

## Executors

- **`RoomEngineClient`** (`runtime/room_engine_client.ex`) - the production
  `EngineClient`. Renders the node's `prompt_ref` through `SymphonyElixir.Prompt`
  (inline text passes through; a `skill` ref is rendered from the active pack's
  skill body with the resolved bindings), appends the run's trigger as an
  `<input>` block, and submits the turn through [`Engine.Client`](contract.md). A
  skill naming an unbound input is a render error, so a half-rendered prompt never
  reaches an engine (`runtime/room_engine_client.ex:17-27`).
- **`ExecRunner`** (`runtime/exec_runner.ex`) - runs one pack shell script in the
  pack directory. The script path resolves relative to the pack so a pack carries
  no absolute paths. Resolved DSL inputs are exported as `SYMPHONY_INPUT_<NAME>`;
  a fresh `SYMPHONY_OUTPUT_FILE` lets a script return a JSON document as the
  node's structured output (which is what makes `when ${node.output.field}` and
  `map ${node.output.items}` work over exec nodes, `runtime/exec_runner.ex:20-26`).
  A finite default timeout (300s) kills a runaway via `kill -KILL`
  (`runtime/exec_runner.ex:176-205`).
- **`SubrunRunner`** (`runtime/subrun_runner.ex`) - a first-class nested run. It
  resolves the child workflow against `WorkflowCatalog`, starts it through
  `Runtime.Ingress.start_workflow/3` under the same supervisor, monitors it, and
  reads the child's terminal `RunGraph` from the store. Two guards bound the tree:
  a cycle guard rejects a name already on the ancestor chain, and a depth ceiling
  rejects a chain past `Config.subrun_max_depth` (default 8) for mutually
  recursive workflows the cycle guard cannot catch (`runtime/subrun_runner.ex:15-31`).

## Placement

`runtime/placement.ex` owns the per-run room-server lifecycle for `:ixvm` and `{:host, _}` agent
placements. A run provisions one room-server before its first agent turn and tears
it down at run end; resolved placements live in a public ETS table keyed by
`run_id` so the off-process `Engine.Client` can read the URL without a GenServer
round-trip (`runtime/placement.ex:21-27`). `:ixvm` runs in a short-lived iXVM via
`ix`; `{:host, _}` runs as a privilege-dropped `systemd-run` unit prefixed
`symphony-host-` (the prefix the polkit grant scopes to). When `:ixvm`
provisioning fails before the first turn, `acquire/3` retries on
`Config.placement_fallback` (default `:host`; also `:remote` to a worker, `:local`
to the in-process server, or `:none`); the fallback registers under the same
`run_id` so the turn resolves without the node knowing it fell back
(`runtime/placement.ex:36-51`). `:local`/`{:room, url}` need no per-run server.
`reconcile/2` reaps host units orphaned by a restart and re-attaches the live ones
(`runtime/placement.ex:205-253`).

## Triggers and ingress

- **`Runtime.Ingress`** (`runtime/ingress.ex`) - the single door from a workflow +
  event to a live run. `start_workflow/3` materializes the AST, stamps the trigger
  onto `graph.trigger`, and starts the run. `start_by_trigger/2` resolves every
  workflow that declared interest (via `WorkflowCatalog.for_trigger_kind/1` +
  `Runtime.Trigger.matches?/2`) and starts one run per match; `seen_trigger?/2` is
  the shared dedup read over the run history in `IR.Store`.
- **`Runtime.Trigger`** (`runtime/trigger.ex`) - the one matcher every producer
  shares: it compares a declared `on` trigger's selector fields (cron schedule,
  Linear label, GitHub repo/label, Slack channel) against an inbound event, so the
  selector vocabulary lives in one module.
- **`Runtime.Events`** (`runtime/events.ex`) - IR-run PubSub. `Runtime` broadcasts
  an `{:ir_run_event, run_id, summary}` after each persisted transition on an
  index topic and a per-run topic, so the dashboard updates without polling.

## Read view (`ir/view.ex`)

`IR.View` renders a `RunGraph` as flat, string-keyed JSON facts for the API and
dashboard: `summary/1` (the index row: status, counts, cost) and `detail/1` (every
node with deps, attempts, output, plus the expansion and audit logs). Keeping the
emitter out of the runtime means a wire-format change never touches scheduling
(`ir/view.ex:4-16`).
