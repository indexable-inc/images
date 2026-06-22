# ix-fleet

`packages/ix-fleet` renders and executes declarative **fleet plans**: a single
JSON document describes a set of remote ix VMs (nodes) and their images, NixOS
switch targets, east-west groups, dependencies, and health checks, and the CLI
converges the live fleet to it. It is the operational front end over the ix
control plane: where [vmkit](../vmkit/overview.md) owns one local guest, `ix-fleet`
manages many remote ix VMs (the SDK calls them branches) by name.

Unlike the rest of this domain, `ix-fleet` does not touch a local hypervisor. It
talks to the ix API through the Python SDK (`ix_sdk.Client`) and fans per-node
work out through [dag-runner](../dag-runner/overview.md). All logic
lives in one module, `packages/ix-fleet/src/ix_fleet/__init__.py`.

## Build and flake output

- Nix-only Python application (not a Rust workspace member). `pyproject.toml`
  declares `name = "ix-fleet"`, the console script `ix-fleet = "ix_fleet:run"`,
  and one runtime dep, `pydantic` (`pyproject.toml:1-11`).
- Built by `packages/ix-fleet/default.nix` with `ix.buildUvApplication`. Flake
  output `ix-fleet` (`package.nix:4`): `nix run .#ix-fleet -- --plan plan.json up`.
- The ix Python SDK is a prebuilt wheel fetched from R2
  ([ix-sdk-python](../ix-sdk-python/overview.md)), copied into the venv at
  `postInstall` rather than resolved by uv (`default.nix:9-11,61-67`).
- The wrapper sets `IX_FLEET_DAG_RUNNER` to the
  [dag-runner](../dag-runner/overview.md) binary (`default.nix:69-70`),
  which the CLI uses to run per-node workflows in parallel.
- Passthru test `dryRunUp` (`default.nix:50-59`) runs
  `ix-fleet --plan <plan> up --skip-push --skip-health --dry-run` in the sandbox:
  it exercises the dry-run control flow (no API, no network) and proves the
  prebuilt SDK wheel imports from the built venv.

## CLI surface (`__init__.py:897-990`)

`ix-fleet --plan <path> [--on NODE_OR_@TAG ...] [--dry-run] <subcommand>`. The
global `--plan` is required; `--on` is repeatable and selects nodes by name or
by `@tag` (empty selects all, in plan order). `--dry-run` prints the steps each
subcommand would run without calling the API.

| subcommand | what it does |
| --- | --- |
| `plan` | Print the resolved node set (dependency-ordered) as JSON. No API calls (`__init__.py:966-968`). |
| `diff` | Print each selected node's desired switch target / source installable (`cmd_diff`, `:726`). |
| `bootstrap` | Create selected nodes from their `bootstrapImage`, wait for guest readiness, and ensure east-west group membership. Runs in dependency batches concurrently (`cmd_bootstrap`, `:881`). |
| `up` | Push each node's replacement image and create-or-replace the node on it, then groups + health. Fans out via dag-runner (`cmd_up`, `:850`). |
| `replace` | Like `up` but always delete-then-create the node on the pushed image (`cmd_replace`, `:836`). |
| `switch` | In-place NixOS system switch of running nodes (target or build-from-source), snapshotting first. Remote source builds go through the platform's native multi-VM `ix up` in dependency layers (`cmd_switch`). |
| `health` | Run each selected node's health checks (`cmd_health`, `:876`). |
| `down` | Remove selected nodes in reverse plan order, collecting failures (`cmd_down`, `:886`). |

`switch` flags: `--no-snapshot`, `--skip-health`, `--source-root`,
`--source-workdir`. `up`/`replace` flags: `--skip-push`, `--skip-health`. The
hidden `_replace-node`/`_up-node` subparsers (`help=argparse.SUPPRESS`) take a
single node positional plus the forwarded flags; they are what dag-runner
invokes per node for `up`/`replace`, not for direct use. `switch` no longer has
a per-node subparser: it batches the switch in-process (see below).

## The plan schema (pydantic, `__init__.py:34-146`)

A plan is a `FleetPlan`: `order` (list of node names), `nodes` (name -> `FleetNode`),
and `secrets` (`FleetSecrets`). `validate_graph` (`:122`) enforces that `order`
has no duplicates, references only defined nodes, and covers every node; that
each node key matches its `name`; and that every `dependsOn` names a real node.

- **`FleetNode`** (`:68`): `name`, `baseName`, optional `replicaIndex`, `system`,
  a `switch` (`SwitchSpec`), `bootstrapImage`, a `replacementImage`
  (`ReplacementImage`), `region`, `ipv4`, `snapshot`, `recreateOnUp` (default
  false), and the lists/maps `tags`, `groups`, `env`, `l7ProxyPorts`,
  `dependsOn`, `healthChecks` (name -> `HealthCheck`).
- **`SwitchSpec`** (`:44`): `target`, `buildOn` (`auto`|`local`|`remote`,
  default `auto`), optional `buildVm`, `sourceInstallable`, `overrideInputs`.
  `sourceInstallable` defaults to the bare node name `.#<node>` (not
  `.#<node>-system`): `mkFleet` exposes a `nixosConfigurations.<node>` output
  (bare external name -> the node's system) so `ix up .#<node>` resolves it, and
  the simple attr lets the native multi-VM `ix up .#a .#b --build-vm <builder>`
  derive each VM name. The `<node>-system` package stays as a build alias. Merge
  the fleet's `nixosConfigurations` into your flake's top-level
  `nixosConfigurations` (see `examples/nixos-switch-multi/flake.nix` for a
  direct `ix up` flake and `examples/dev-fleet/default.nix` for the mkDev
  wrapper).
- **`ReplacementImage`** (`:34`): `imageName`, `imageTag`, `destination`,
  `source`, `sourceDrv` (the OCI image derivation to realise and push).
- **`HealthCheck`** (`:54`): `description`, `command` (argv), `timeoutSec`,
  `attempts`, `intervalSec`, `requiresIpv4`, and `from` (`guest`|`host`, stored
  as `from_` since `from` is a keyword).
- **`FleetSecrets`** (`:104`): a `provider` (`type`, `mountRoot`, plus arbitrary
  extra keys) and `values` (name -> `SecretSpec`: `key`, `path`, extra keys).
  Default provider is `runtime-directory` at `/run/secrets` (`:116-120`).

## Backend: the ix SDK control plane

The CLI drives the ix API through `ix_sdk.Client`, constructed lazily so
`--dry-run` never needs a token (`client()`, `:200-210`). The client resolves
`IX_TOKEN` and the base URL from the environment, the same inputs the `ix` CLI
uses. A node maps to an ix branch (`ix_sdk.BranchInfo`/`BranchStatus`:
`RUNNING`/`STOPPED`/`FAILED`). Key SDK calls:

- create / delete / start a branch: `client().create(...)`, `branch.delete()`,
  `branch.start()` (`create_node` `:366`, `remove_node` `:509`).
- image push: `client().image_push(source, destination, region)` after a
  host-side `nix-store --realise` of `sourceDrv` (`push_replacement_image`, `:338`).
- snapshot: `client().snapshot(name=...)` (`snapshot_node`, `:423`).
- in-place switch: `client().switch_system(name, target, build_on)`
  (`switch_node`, `:430`).
- east-west groups: `client().create_group(...)` / `add_group_member(...)`,
  idempotent on `ix_sdk.IxConflictError` (`ensure_group` `:461`,
  `ensure_node_groups` `:472`).

Image swap is **delete-then-create**, not in-place: `client.create` inserts
against a UNIQUE (owner, name) constraint, so changing a node's image means
`recreate_node` = remove + create (`recreate_node`, `:383-393`). In-place updates
are what `switch` is for.

## Workflows and ordering

- **Node selection is dependency-ordered.** `selected_nodes` (`:167`) returns
  the selected nodes with each node's `dependsOn` emitted before it (a DFS over
  plan order), and `dependency_batches` (`:489`) groups them into parallel
  batches for `bootstrap`.
- **`up`/`replace` fan out through dag-runner.** `run_node_workflow_dag`
  builds a spec `{"nodes": {name: {command, depends_on}}}` where each command
  re-invokes `ix-fleet ... _<verb>-node <name>` with the forwarded flags; it also
  adds a serialization edge between nodes that share a
  `replacementImage.destination` so the same image tag is not pushed twice
  concurrently. `run_dag_runner` resolves the runner from `IX_FLEET_DAG_RUNNER`
  or `dag-runner` on `PATH`, writes a temp spec, execs it, and turns a nonzero
  exit into the process exit status. `--dry-run` runs the per-node workflows
  inline instead (so child output is visible).
- **`switch` batches in-process through the native multi-VM `ix up`.**
  `cmd_switch` walks `dependency_batches` layers in order (so `dependsOn` still
  gates the switch) and, within each layer, groups the batchable nodes by
  `(buildVm, overrideInputs)` and runs one `ix up .#a .#b --build-vm <builder>`
  per group (`switch_nodes_from_source`); the platform builds every closure on
  the one warm builder and activates each on its own VM. Each group's VMs are
  pre-created with their full fleet config and snapshotted first
  (`switch_group_workflow`), then the batch only switches existing VMs. Groups
  and single-node fallbacks within a layer run concurrently via `asyncio.gather`.
- **Per-node workflows.** `run_up_node_workflow`: push image (unless
  `--skip-push`) -> `up_node` (create if absent, else recreate on the uploaded
  image) -> ensure groups -> health (unless `--skip-health`).
  `run_replace_node_workflow` is the same with an unconditional recreate.
  `run_switch_node_workflow` (the single-node fallback): ensure the node exists
  -> ensure groups -> snapshot (if `node.snapshot` and not `--no-snapshot` and
  the node was not just created) -> switch -> health.
- **Switch picks a path per node.** A node is batched into the native multi-VM
  `ix up` only when it builds remotely, names a `buildVm`, and its installable is
  exactly `.#<node-name>` (`is_batchable_switch`); the multi `ix up` derives each
  VM name from that attr and rejects `--name`. Anything else falls back to the
  single-target `ix up <installable> --name <node>` (`switch_node_from_source`,
  `buildOn == "remote"` without a build VM or with a custom installable) or the
  SDK `switch_system` (`switch_node`, `buildOn` `local`/`auto`), bounded by
  `SWITCH_TIMEOUT_SECS = 1800`. Both source paths retry a transient
  `stream framing error` up to `MAX_SWITCH_RETRIES` (`run_source_switch`).

## Health checks (`__init__.py:540-624`)

Each `HealthCheck` runs up to `attempts` times with `intervalSec` between tries:

- **guest** (`from: guest`): run the check argv inside the VM through the SDK
  exec channel (`branch.exec`), bounded by `timeoutSec`.
- **host** (`from: host`): run on the operator's machine via subprocess, with
  `IX_NODE_*` env (`node_env_vars`, `:520`: `IX_NODE`, `IX_NODE_NAME`,
  `IX_NODE_IMAGE`, `IX_NODE_STATUS`, `IX_NODE_IPV6`, `IX_NODE_IPV4`,
  `IX_NODE_SUBDOMAIN`, `IX_NODE_REGION`) substituted into the command via
  `string.Template`. `requiresIpv4` gates the check until the node reports an
  IPv4 address.

## Bootstrap readiness

`wait_node_ready` (`:313`) polls (180s deadline) by running a probe script in
the guest via `branch.bash` that starts `nix-daemon.socket` and runs
`nix ... store info`; the node is ready when the probe exits 0. This is what
`bootstrap` and the create paths wait on before proceeding.

## Errors

`run()` (`:993`) wraps `main` and maps `OSError`, `ValidationError`,
`ValueError`, `TypeError`, `RuntimeError`, and `CalledProcessError` to a
`ix-fleet: <msg>` line on stderr and `SystemExit(1)`. Shelled commands raise
`CliError`/`CliTimeoutError` (`:223-246`) carrying the captured output.
