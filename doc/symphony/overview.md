# Symphony

Symphony is a boring DAG runtime for deterministic agent workflows. It is an
Elixir/OTP control plane (`packages/agent/symphony/elixir`) that orchestrates Codex
and Claude agent sessions across one or more git repositories. Workflows are
authored in the `.sym` surface language, lowered to an intermediate-representation
(IR) run graph, and walked by a supervised per-run GenServer with a LiveView
dashboard, cron/Slack/Linear/GitHub triggers, and per-run git worktrees. The
single Nix flake output is `nix run .#symphony` (`packages/agent/symphony/default.nix:148`);
the runtime drives a Rust `room-server` (in the ix monorepo, not here) over HTTP
to actually run each agent turn.

Read this page first, then the component pages it links. Symphony moved here from
the standalone `indexable-inc/symphony` repo at rev `c9e7092`
(`packages/agent/symphony/README.md:10`).

## Units

This domain is one package (`packages/symphony`) whose Elixir source is layered.
The layers, and where each is documented:

| layer | path | role |
| --- | --- | --- |
| DSL | `elixir/lib/symphony_elixir/dsl/` | the `.sym` lexer, parser, reified AST, interpreter (eval-as-emission), and schema. See [dsl](dsl/overview.md). |
| IR + runtime | `elixir/lib/symphony_elixir/ir/`, `runtime/` | the durable `RunGraph`, the per-run scheduler, executors, crash recovery, and per-run room-server placement. See [engine](engine/overview.md). |
| engine wire | `elixir/lib/symphony_elixir/engine/` | the typed `Engine.Envelope` and the `Engine.Client` that lowers a turn to the room-server `TurnRequest`. See [engine contract](engine/contract.md). |
| pack | `elixir/lib/symphony_elixir/{catalog,workflow_catalog,skill,prompt}.ex`, `repository_catalog.ex`, `workflows/example/` | the hot-reloaded workflow-pack format (`.sym` + skills + `repositories.yaml`). See [pack](pack/overview.md). |
| operations | `default.nix`, `bin/run-nix`, `config.ex`, `*_web/`, triggers, `../../modules/services/symphony` | launching, env vars, the dashboard at `:4040`, triggers, and the NixOS module. See [operations](operations/overview.md). |

It is a Nix-only package (a Nushell launcher wrapping `bin/run-nix`), not a Rust
workspace member. The `room-server` it drives is a separate package in the ix
monorepo (`crates/room`, `packages/room`); only the Elixir control plane lives
here (`packages/agent/symphony/default.nix:9-13`).

## How it fits together

```
.sym source --parse--> reified AST --interpret/expand--> IR RunGraph --schedule--> agent turn
  DSL.Parser            DSL.AST          DSL.Interpreter      IR.*           Engine.Client -> room-server
```

The layering is a strict one-way dependency the overhaul encodes:
`DSL -> IR -> Runtime -> Engine.Client -> room-server`, with `Engine.Client`
the only module in `elixir/lib/` that names the room-server HTTP contract
(`engine/client.ex:1-8`). A producer (cron, a webhook, the dashboard) resolves
a trigger event to a `WorkflowCatalog` entry, `Runtime.Ingress` materializes its
AST into a `RunGraph`, and `Runtime.Supervisor` starts one `Runtime` GenServer
that schedules ready nodes as monitored BEAM tasks until every node is terminal.

- A workflow is a `do`-block of statements. A `name <- effect` binding introduces
  a data dependency; statements whose inputs do not reference each other have no
  edge and run in parallel (`dsl/ast.ex:15-28`). The graph gets auto-parallelism
  from data flow, never a `needs:` list.
- Only effectful constructors (`agent`, `exec`, `subrun`) become `IR.Node`s. Pure
  values (string concat, `${node.field}` reads) are evaluated inside the
  interpreter at expand time and never fill the graph with trivial nodes
  (`dsl/interpreter.ex:22-27`).
- Each agent node carries an `Engine.Envelope` (engine/model/effort/permissions/
  location), validated and lowered at load (`ir/materializer.ex:41-45`).

## Invariants

- **Determinism by replay.** A run is durable as a `RunGraph` (`ir/run_graph.ex`):
  the reified AST, the materialized nodes, and an append-only `expansion_log`. On
  restart the runtime does not resurrect a live computation; it re-runs the
  interpreter against the recorded outputs and log, rebuilding the identical node
  set. The asserted invariant is `replay(ast, expansion_log) == nodes`
  (`ir/run_graph.ex:14-19`). Gates (`when`, `every`, `map`) are pure functions of
  `known_outputs` and persisted counters: no wall clock, no RNG
  (`dsl/interpreter.ex:35-39`).
- **Edges are derived, never declared.** `IR.Node.deps` is computed from `inputs`
  (`ir/node.ex:170-180`); an input that reads another node's output is the only
  thing that makes an edge. A node is ready only when every dep `:succeeded`
  (`ir/graph.ex:56-65`).
- **Crash bias is conservative.** Agent turns are not idempotent (a turn may push
  a commit), so a node stranded by a task/BEAM crash is auto-retried only if it
  opted in AND showed no side effect (no `thread_id` recorded); otherwise it is
  left `:stranded` for human review (`runtime/recovery.ex:23-41`).
- **Workspace safety.** Every run gets a fresh `git worktree` under
  `SYMPHONY_WORKSPACES_DIR`; an agent turn never runs with cwd inside the source
  repo, and workspace-relative paths route through `PathSafety.canonicalize/1`
  (`workspace.ex:66-85`).
- **Pack-agnostic core.** No workflow names, repo slugs, labels, or ticket schemes
  are hardcoded in `elixir/lib/`; workflow shape lives in the pack
  (`packages/agent/symphony/AGENTS.md:29-35`).

## Glossary

- **`.sym`**: the surface workflow language, parsed to a reified AST
  (`dsl/parser.ex`). See [dsl](dsl/overview.md).
- **effect**: an `agent`, `exec`, or `subrun` constructor; the only kinds that
  become IR nodes (`dsl/ast.ex:176-180`).
- **combinator / gate**: `when`, `every n of <counter>`, `map ... as`: dynamic
  constructs that emit a placeholder node, then emit their body deterministically
  once the gating input resolves (`dsl/interpreter.ex:31-34`).
- **eval-as-emission**: evaluating the AST against known outputs emits IR nodes;
  re-evaluating with more outputs emits the next delta (`ir/materializer.ex:13-28`).
- **RunGraph**: the durable per-run state (AST, nodes, expansion log)
  persisted as JSON by `IR.Store` (`ir/run_graph.ex`).
- **expansion log**: the append-only record of each dynamic expansion; replaying
  it rebuilds the materialized graph (`ir/run_graph.ex:7-19`).
- **envelope**: a node's typed execution spec (engine, model, effort, permissions,
  location), validated by `Engine.Envelope` (`engine/envelope.ex`).
- **placement / location**: where an agent's engine process runs (`:local`,
  `:ixvm`, `{:host, _}`, `{:room, url}`); the run provisions its own room-server
  for `:ixvm`/`:host` (`runtime/placement.ex`).
- **room-server**: the Rust engine host (ix monorepo) that runs one agent turn
  over `POST /api/agent/turns`; Symphony speaks to it only through `Engine.Client`.
- **pack**: a directory of `workflows/*.sym`, `skills/*.md`, and
  `repositories.yaml` selected with `SYMPHONY_PACK_DIR` (`config.ex:16-24`).
- **skill**: a markdown system prompt under `skills/`, model-agnostic, referenced
  by an agent node as `prompt: skill "name"` (`skill.ex:1-27`).

## Components

| component | page | what |
| --- | --- | --- |
| dsl | [dsl/overview.md](dsl/overview.md) | the `.sym` language: lexer, grammar, AST, node types, triggers, fields, interpreter |
| engine | [engine/overview.md](engine/overview.md) | the IR run graph and the supervised DAG runtime: scheduling, executors, recovery, placement |
| engine contract | [engine/contract.md](engine/contract.md) | the cross-language wire seam: `Envelope`, `TurnRequest`/`EngineEvent`/`AgentTurnResponse`, golden fixtures |
| pack | [pack/overview.md](pack/overview.md) | the workflow-pack format (workflows, skills, repositories.yaml), hot-reload catalogs, the bundled example |
| operations | [operations/overview.md](operations/overview.md) | running it: `nix run .#symphony`, env vars, placements, the dashboard at `:4040`, triggers, the NixOS module |
