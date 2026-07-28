# dag-runner

`packages/dag-runner` is a tiny task runner: it takes a JSON DAG of shell
commands, runs each node as soon as its dependencies finish (so graph shape, not
fixed "levels", determines parallelism), and renders inline progress. It powers
`nix run .#health-checks` and is the planned replacement for `ix-fleet`'s
sequential per-node loops (`README.md`). It is meant for short, hands-off
batches, not a long-running supervisor; rationale (why not `process-compose`/
`devenv-tasks`) is in the `why-dag-runner` skill
(`skills/why-dag-runner/SKILL.md`).

It is a Tokio current-thread async binary (`src/main.rs:142`).

```
dag-runner <spec.json> [--output auto|tui|plain|json] [--only NAMES]
```

## Spec schema

A single JSON object with a `nodes` map (`Spec`/`NodeSpec`, `src/main.rs:69-89`).
Each node:

| field | type | required | meaning |
| --- | --- | --- | --- |
| `command` | `string[]` | yes | argv; `command[0]` is the program. Must be non-empty. |
| `depends_on` | `string[]` | no (`[]`) | names of nodes that must succeed first. |
| `env` | `{string:string}` | no (`{}`) | extra env layered on the runner's own; entries shadow inherited vars. |
| `timeout_secs` | `u64` | no (`null`) | wall-clock seconds before SIGTERM (then SIGKILL after ~500ms). |

Validation runs before any node is spawned (`validate`, `src/main.rs:208-221`)
and rejects an empty `command`, a `depends_on` naming an unknown node, and cycles
(direct or indirect) via a three-color DFS that prints the cycle path
(`detect_cycle`/`visit_cycle`, `src/main.rs:262-296`).

`--only NAMES` (comma-separated, repeatable) restricts the run. The cut is
validated up front (`filter_only`, `src/main.rs:226-260`): unknown names are
rejected, and a kept node depending on a dropped one is rejected too, so a
filtered run keeps the same "every kept node has every dep it needs" invariant
(no silent skips, no exit-code surprises).

## Scheduling and execution

Spawn order is the deterministic topological order of the graph, with ties broken
lexicographically so logs are stable across runs
(`topological_order`/`visit_topo`, `src/main.rs:298-326`). Each node becomes a
`Shared<BoxFuture<Outcome>>` that awaits its dependency futures, then either skips
(if any dep did not `Succeeded`) or spawns the command; all node futures are
`tokio::spawn`ed and awaited (`run`, `src/main.rs:330-423`). Independent nodes run
concurrently.

`run_command` (`src/main.rs:440-531`) builds a `tokio::process::Command` with the
env overlay, piped stdout/stderr, and `kill_on_drop(true)` (so a dropped future
never leaks a child). It then races, `biased`, in one `select!`:
cancellation, then the optional timeout, then `child.wait()`
(`src/main.rs:480-494`). Outcomes (`Completion` -> `Outcome`):

- success -> `Succeeded`.
- non-zero exit -> `Failed(code)`.
- spawn failure (binary missing) -> `Failed(127)`.
- timeout -> `terminate_child` then `Failed(124)` (matches `coreutils timeout`),
  with stderr ending `dag-runner: node timed out after Ns`.
- cancellation -> `terminate_child` then `Failed(130)`.

`terminate_child` (`src/main.rs:571-588`) sends `SIGTERM` via
`libc::kill` (Tokio only exposes `SIGKILL` through `start_kill`), waits a 500ms
grace, then `SIGKILL`s if still alive. Child stdout/stderr is captured (not
streamed) by `tee_lines` (`src/main.rs:593-613`), which in TUI mode also updates
the node's spinner with the latest line.

## Cancellation

A background task listens for Ctrl-C (`spawn_cancel_listener`,
`src/main.rs:179-193`): the first SIGINT broadcasts a `watch` cancel flag (every
running child is SIGTERMed then SIGKILLed); a second SIGINT hard-exits 130
immediately. A cancellation always exits 130 even if every node already finished,
so callers distinguish operator-cancel from normal completion
(`main`, `src/main.rs:169-175`).

## Output modes

`resolve_mode` (`src/main.rs:195-206`): `auto` picks `tui` on a TTY, else
`plain`.

- **tui**: an indicatif `MultiProgress` with one inline spinner per node (style
  from the `progress-style` crate via `progress_style::spinner()`,
  `src/main.rs:425-431`). Finished spinners stay in scrollback.
- **plain**: timestamped `started` / `<outcome>` lines.
- **json**: NDJSON event stream on stdout, three shapes discriminated by `event`
  (`Event`, `src/main.rs:115-135`): `node_started` (`ts_ms`), `node_finished`
  (`outcome`, `exit_code`, `duration_ms`), and a final `summary`. Ordering
  guarantees: a node's `node_started` precedes its `node_finished` and does not
  appear until every dependency's `node_finished` has; `summary` is last.

In every non-json mode, after all nodes settle a one-line summary plus per-node
breakdown and the captured stdout/stderr of failed nodes go to stderr
(`print_summary`, `src/main.rs:719-756`).

## Exit code

`exit_code` (`src/main.rs:704-717`) = `max(worst node exit code, 1 if any node
was skipped, else 0)`. Skipped counts as 1; a failure with exit `N` contributes
`N`; multiple failures take the largest code. Operator Ctrl-C overrides to 130.

## How it is built

`default.nix` selects the `dag-runner` binary via
`ix.cargoUnit.selectBinaryWithTests` (flake output `dag-runner`, `package.nix`).
Note the renamed integration test target: cargo-unit flattens test targets into
one namespace, so a bare `integration` name would collide with other crates'; the
crate names it `integration_dag_runner` for a stable key
(`Cargo.toml:27-33`). `tests/integration.rs` plus inline unit tests cover the
graph semantics.
