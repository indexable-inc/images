# Running a fleet

A fleet is a set of remote ix VMs (nodes) managed as one unit from a single
declarative plan. You author the fleet in Nix with `index.lib.mkFleet`
(`lib/image/fleet.nix:19-24`), which renders a JSON plan and exposes a runnable
command per lifecycle verb; `ix-fleet` (`packages/ix-fleet`, Python) reads that
plan and converges the live fleet to it. This page is the short how-to; the
exhaustive from-source reference is [../ix-fleet/overview.md](../ix-fleet/overview.md).

> **Deprecated.** Fleets are removed from the ix product surface
> (indexable-inc/ix#8306): the model is one VM per project, defined by your
> config and converged with `ix apply`. The standalone `ix-fleet` tool still
> lives in this repo, and this page remains its how-to.

> `ix-fleet` is a separate tool from the [`ix` CLI](cli.md). It drives the same
> ix control plane (a node is an ix branch) through the Python SDK and `IX_TOKEN`
> from the environment, but you do not call `ix` directly to run a fleet.

## The flow: mkFleet -> plan -> up

You rarely write a plan by hand. You write a `mkFleet` spec; it renders the plan
and gives you the verbs. A minimal real example
(`examples/nginx-lifecycle/default.nix`):

```nix
{ index }:

index.lib.mkFleet {
  nodes.nginx = {
    deployment.recreateOnUp = true;
    modules = [ ./service.nix ];
  };
}
```

A multi-node fleet adds `groups`, `replicas`, and `dependsOn`
(`examples/ray-cluster/default.nix`):

```nix
nodes = {
  ray-head = { groups = [ "ray-cluster" ]; modules = [ ./head.nix ]; };
  ray-worker = {
    replicas = 2;
    dependsOn = [ "ray-head" ];
    groups = [ "ray-cluster" ];
    modules = [ ./worker.nix ];
  };
};
```

`mkFleet` returns, per node spec, a rendered `plan` plus one wrapped command per
verb (`up`, `switch`, `replace`, `bootstrap`, `health`, `status`, `logs`,
`diff`, `down`); each
command bakes in `--plan` so you run `nix run .#up` rather than passing the plan
yourself (`lib/image/fleet.nix:405-426`). It also exposes
`nixosConfigurations.<node>` (the bare node name -> that node's system) so
`ix apply .#<node>` resolves it; merge it into your flake's top-level
`nixosConfigurations` (`lib/image/fleet.nix:436-443`, see `templates/dev/flake.nix`).

To run a verb against the raw tool instead of the wrapper:
`nix run .#ix-fleet -- --plan plan.json up` (`package.nix:4`). For all flags, run
`nix run .#ix-fleet -- --help`.

## Subcommands

`ix-fleet --plan <path> [--on NODE|@TAG ...] [--dry-run] <subcommand>`. `--plan`
is required; `--on` is repeatable and selects nodes by name or `@tag` (empty =
all, in plan order); `--dry-run` prints the steps without calling the API
(`__init__.py:897-990`).

| verb | what it does |
| --- | --- |
| `plan` | Print the resolved, dependency-ordered node set as JSON. No API calls. |
| `diff` | Print each selected node's desired switch target / source installable. |
| `bootstrap` | Create nodes from their `bootstrapImage`, wait for guest readiness, ensure groups. |
| `up` | Push each node's image and create-or-replace the node on it, then groups + health. |
| `replace` | Like `up`, but always delete-then-create the node on the pushed image. |
| `switch` | In-place NixOS system switch of running nodes (snapshots first). |
| `health` | Run each selected node's health checks. |
| `status` | One row per node: live STATUS, READY (health checks passed/total), ADDRESS. `-o wide` adds REGION and running vs desired IMAGE; `-o json` for scripts; `--watch [--interval N]` polls until interrupted; `--no-checks` skips probes. In one-shot mode, exits 1 if any node is unhealthy. |
| `logs` | Pull journalctl output from selected nodes (`-u/--unit`, `-n/--lines`, `--since`), prefixing each line with `[node]` when more than one node is selected. |
| `down` | Remove selected nodes in reverse plan order. |

For up vs switch vs replace vs down and when to use each, see
[lifecycle.md](lifecycle.md).

## Node and plan fields (brief)

You write the **authoring** surface (mkFleet); `ix-fleet` consumes the rendered
**plan** surface. Both are listed below; the full field reference with types and
defaults is in [../ix-fleet/overview.md](../ix-fleet/overview.md).

Authoring (per node spec): `modules`/`module`, `deployment`, `tags`, `groups`,
`dependsOn`, `replicas`, `updateStrategy` (`lib/image/fleet.nix:109-114`).
`updateStrategy.maxUnavailable = k` (positive int; the only key) rolls a
replicated node's replicas Kubernetes-RollingUpdate style: `up`/`replace`
recreate at most `k` replicas concurrently, each gated on its health checks
before the next starts (`lib/image/fleet.nix:139-165`). `deployment` accepts
`bootstrapImage`, `destination`, `env`, `ipv4`, `l7ProxyPorts`,
`noDefaultSecrets`, `recreateOnUp`, `region`, `secrets`, `sources`, `switch` -
and only those; unknown keys error (`lib/image/fleet.nix:77-102`). Health checks
are not a deployment key: declare them in a node module as `ix.healthChecks.<name>`
(`lib/image/fleet.nix:72-76`).

Rendered plan (pydantic, `__init__.py:34-146`): a `FleetPlan` is `order`, `nodes`
(name -> `FleetNode`), `secrets`. A `FleetNode` carries `name`, `system`,
`switch` (`SwitchSpec`: `target`, `buildOn` auto|local|remote, `buildVm`,
`sourceInstallable`, `overrideInputs`), `bootstrapImage`, `replacementImage`,
`region`, `ipv4`, `snapshot`, `recreateOnUp`, `tags`, `groups`, `env`,
`l7ProxyPorts`, `dependsOn`, `healthChecks`, `secrets`, `sources`,
`noDefaultSecrets`, `updateStrategy`.

## See also

- [lifecycle.md](lifecycle.md) - up vs switch vs replace vs down.
- [health-checks.md](health-checks.md) - declaring and running checks.
- [secrets.md](secrets.md) - the secret-ref model.
- [networking.md](networking.md) - east-west groups and L7 proxy ports.
- [images.md](images.md) - bootstrap vs replacement images, the delete-then-create swap.
- [../ix-fleet/overview.md](../ix-fleet/overview.md) - the full from-source reference.
- [cli.md](cli.md) - the `ix` CLI (distinct from `ix-fleet`).
