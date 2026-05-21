# dag-runner

A tiny task runner that takes a JSON DAG of shell commands, runs nodes in parallel as their dependencies finish, and renders inline progress. It powers `nix run .#health-checks` today and is the planned replacement for [`ix-fleet`](../ix-fleet/)'s sequential per-node loops.

The runner is meant for short, hands-off batches: spawn a fan-out of independent jobs, follow their progress, and exit with a worst-case status. It is not a long-running supervisor. For the design rationale (why not `process-compose`, why not `devenv-tasks`), see [the corresponding AGENTS.md section](../../AGENTS.md#why-dag-runner-and-not-process-compose-or-devenv-tasks).

## Usage

```
dag-runner <spec.json> [--output auto|tui|plain|json] [--only NAMES]
```

`--output auto` (default) picks `tui` when stdout is a TTY and `plain` otherwise. `json` emits NDJSON events to stdout and a final `summary` line; everything else still goes to stderr.

`--only` restricts the run to the named nodes (comma-separated, repeatable: `--only a,b --only c`). Unknown names and edges left dangling by the cut (a kept node depending on a dropped one) are rejected before any node is spawned, so a filtered run keeps the same "every kept node has every dep it needs" invariant as an unfiltered run.

## Spec schema

The spec is a single JSON object with a `nodes` map. Each entry is a node keyed by name.

| field | type | required | meaning |
| --- | --- | --- | --- |
| `command` | `string[]` | yes | argv. `command[0]` is the program, the rest are arguments. Must be non-empty. |
| `depends_on` | `string[]` | no, default `[]` | Names of other nodes that must succeed first. |
| `env` | `{string: string}` | no, default `{}` | Extra env vars layered on top of the runner's own env. Entries here shadow inherited vars; missing entries are inherited from the parent. |
| `timeout_secs` | `u64` | no, default `null` | Wall-clock seconds before the child is SIGTERMed (then SIGKILLed after ~500ms grace). On expiry the outcome is `failed` with exit code `124` (matches `coreutils timeout`) and the captured stderr ends with `dag-runner: node timed out after Ns`. |

Validation runs before any node is spawned and rejects:

- An empty `command` array.
- A `depends_on` entry that names an unknown node (error names both nodes).
- A cycle, direct (`a → a`) or indirect (`a → b → c → a`). The error shows the cycle path.

Nodes are spawned in topological order; siblings without a dependency relationship may run concurrently. When the runner has to break ties (independent roots, or siblings inside one layer), it walks names in lexicographic order so logs stay stable across runs.

## Example

```json
{
  "nodes": {
    "fetch":   { "command": ["curl", "-fsSL", "https://example.test/data.json", "-o", "data.json"] },
    "lint":    { "command": ["jq", ".", "data.json"], "depends_on": ["fetch"] },
    "convert": { "command": ["./bin/convert", "data.json", "out.bin"], "depends_on": ["fetch"], "env": { "RUST_LOG": "debug" } },
    "upload":  { "command": ["./bin/upload", "out.bin"], "depends_on": ["lint", "convert"] }
  }
}
```

`lint` and `convert` run in parallel after `fetch`. `upload` waits for both. A failure in `fetch` propagates: `lint`, `convert`, and `upload` all end up `skipped`.

## Output modes

- **`tui`**: an indicatif `MultiProgress` with one inline spinner per node. Spinners stay in scrollback after they finish, so a failure leaves its line visible. Live stdout/stderr from each child is captured (not streamed) and dumped at the end for failed nodes.
- **`plain`**: timestamped `started` / `<outcome>` lines to stdout. No spinners, no alt-screen.
- **`json`**: NDJSON event stream on stdout. See below.
- **`auto`**: `tui` when stdout is a TTY, `plain` otherwise.

In every mode, after all nodes settle, a one-line summary plus a per-node breakdown (and captured stdout/stderr for any failed nodes) is written to stderr.

## `--output json` event schema

One JSON object per line. Three event shapes, discriminated by `event`:

```json
{ "event": "node_started",  "node": "fetch", "ts_ms": 12 }
{ "event": "node_finished", "node": "fetch", "outcome": "succeeded", "exit_code": null, "duration_ms": 412 }
{ "event": "node_finished", "node": "lint",  "outcome": "failed",    "exit_code": 1,    "duration_ms": 87  }
{ "event": "node_finished", "node": "upload","outcome": "skipped",   "exit_code": null, "duration_ms": 87  }
{ "event": "summary", "total": 4, "succeeded": 1, "failed": 1, "skipped": 2, "duration_ms": 510 }
```

| field | type | notes |
| --- | --- | --- |
| `node` | string | Node name from the spec. |
| `ts_ms` | u128 | Milliseconds since the runner started (only on `node_started`). |
| `outcome` | `"succeeded"` \| `"failed"` \| `"skipped"` | Final state. `skipped` means one of its dependencies did not succeed. |
| `exit_code` | i32 \| null | Set when `outcome == "failed"`. `null` otherwise. A spawn error (binary missing, etc.) surfaces as `outcome: "failed"` with `exit_code: 127`. |
| `duration_ms` | u128 | On `node_finished`, time the runner spent on that node (from spawn to exit, or zero for skipped). On `summary`, total wall-clock time. |

Ordering guarantees:

- For one node, `node_started` always precedes its `node_finished`.
- A node's `node_started` does not appear until every dependency's `node_finished` has been emitted.
- Independent nodes run concurrently. Their `node_started` and `node_finished` lines may interleave in any order between nodes.
- `summary` is the final line.

## Exit code

```
exit_code = max(worst node exit code, 1 if any node was skipped, else 0)
```

Concretely:

- Empty spec or every node succeeded: `0`.
- One node failed with exit `N`: `N`.
- Multiple failures: the largest non-zero `exit_code` across failed nodes wins.
- At least one node was skipped (because a dep failed) and no failure had a larger code: `1`.
- A node could not be spawned: counted as `failed` with `exit_code = 127`.
- A node hit its `timeout_secs`: counted as `failed` with `exit_code = 124`.
- The operator hit Ctrl-C: every running child is SIGTERMed (then SIGKILLed after ~500ms grace), and the runner exits `130` regardless of which nodes had already finished. A second Ctrl-C hard-exits immediately.

CI pipelines should treat any non-zero exit as a stop signal and read stderr for the per-node breakdown and captured child output.
