# Declarative fleets

`index.lib.mkFleet` evaluates a set of NixOS nodes into the data consumed by
`ix up`. A fleet flake exposes the result at `ix.fleets.default` and exposes
its systems at `nixosConfigurations`:

```nix
outputs = {index, ...}: let
  fleet = index.lib.mkFleet {
    nodes = {
      db = ./db.nix;
      web = {
        dependsOn = ["db"];
        deployment.ipv4 = true;
        modules = [./web.nix];
      };
    };
  };
in {
  ix.fleets.default = fleet;
  inherit (fleet) nixosConfigurations;
};
```

From that flake directory, deploy the whole fleet with:

```sh
ix up
```

`ix up` uploads the repository once, creates or starts the declared VMs,
builds each `nixosConfigurations.<node>` on its own target VM, activates it,
and reconciles dependencies, groups, secrets, and health gates. The caller
does not build a system closure and there is no user-selected builder VM.

## Authoring surface

Fleet-wide `defaults` are NixOS modules applied to every node. A node may be a
module directly or an attrset with `modules`, `deployment`, `tags`, `groups`,
`dependsOn`, `replicas`, and `updateStrategy`.

Deployment data supports `bootstrapImage`, `region`, `ipv4`, `snapshot`,
`env`, `secrets`, and `l7ProxyPorts`. Health checks come from
`ix.healthChecks.<name>` in the evaluated NixOS configuration.

`mkFleet` returns:

- `planValue`: serializable topology and deployment data for `ix up`.
  `schemaVersion = 1` is the typed ix/index compatibility boundary; `ix up`
  rejects versions it does not understand.
- `nodes`: evaluated node configurations for Nix authors.
- `meta`: normalized node specifications.
- `nixosConfigurations`: the systems `ix up` realizes on their target VMs.

See [health-checks.md](health-checks.md), [secrets.md](secrets.md), and the
[`fleet/hello`](../../examples/fleet/hello/) example.
