# Services

`modules/services/` ships ready-made NixOS service modules you can drop into an
ix image. Each is a self-contained module under `modules/services/<name>/` that
exposes a single `enable` option (plus per-service tuning options). Turning one
on installs the systemd units, packages, and defaults for that workload, and
most services also declare their own `ix.networking` port claims and
`ix.healthChecks` so the fleet knows how to reach and probe them. This page is
an index: pick a service, read its module for the full option set, and set its
verified enable option to `true`. Option names are NOT uniform: some live under
`services.<name>`, others under `services.ix-<name>`. Use the exact option in
the table below.

## Available services

| Service | What it is | Enable option (verified `path:line`) |
| --- | --- | --- |
| ci-runner | Self-hosted GitHub Actions runners for this repo on a persistent NixOS host, reusing the host `/nix/store` and Cachix substituter for warm builds. | `services.ci-runner.enable` (`modules/services/ci-runner/default.nix:29`) |
| floodgate | Floodgate auth bridge installed as a Velocity plugin beside Geyser (lets Bedrock players join without a Java account). | `services.floodgate.enable` (`modules/services/floodgate/default.nix:59`) |
| geyser | Geyser Bedrock-to-Java protocol bridge, installed as a Velocity plugin. | `services.geyser.enable` (`modules/services/geyser/default.nix:78`) |
| git-clone | Clones a git repository on first boot, idempotently (later boots see `.git` and do nothing). | `services.git-clone.enable` (`modules/services/git-clone/default.nix:21`) |
| humanlayer | Runs the HumanLayer (riptide) remote daemon as a long-lived systemd service so an ix VM acts as a remote HumanLayer host. | `services.humanlayer.enable` (`modules/services/humanlayer/default.nix:46`) |
| minecraft | Loader-agnostic Java Minecraft server runtime (systemd unit, mods, Java, port; `serverJar`/`dropinDir` filled by a loader module). | `services.minecraft.enable` (`modules/services/minecraft/default.nix:809`) |
| minecraft-bedrock | Minecraft Bedrock Dedicated Server (native Linux, kept separate from the Java loader family). | `services.minecraft-bedrock.enable` (`modules/services/minecraft-bedrock/default.nix:63`) |
| minestom | Minestom server runtime that runs a user-built fat jar (no loaders, mods, or EULA). | `services.minestom.enable` (`modules/services/minestom/default.nix:40`) |
| observability | Self-hosted OpenTelemetry, ClickHouse, and Grafana stack. | `services.ix-observability.enable` (`modules/services/observability/default.nix:114`) |
| postgresql | PostgreSQL 18 with performance-tuned defaults for AMD EPYC Gen 5 (Zen 5). | `services.ix-postgresql.enable` (`modules/services/postgresql/default.nix:21`) |
| ray | Ray cluster node plus the ix-mcp engine that drives the `fleet` distributed API. | `services.ix-ray.enable` (`modules/services/ray/default.nix:195`) |
| remote-desktop | Browser-accessible remote desktop backed by Xpra's built-in HTML5 client. | `services.remote-desktop.enable` (`modules/services/remote-desktop/default.nix:71`) |
| resource-monitor | Browser-accessible VM resource monitor. | `services.resource-monitor.enable` (`modules/services/resource-monitor/default.nix:127`) |
| seaweedfs | Single-node S3-compatible object storage via SeaweedFS (`weed server -s3`). | `services.ix-seaweedfs.enable` (`modules/services/seaweedfs/default.nix:55`) |
| spark | Apache Spark standalone cluster shipping the Gluten + Velox native execution engine by default. | `services.ix-spark.enable` (`modules/services/spark/default.nix:193`) |
| subagent-cache | Content-validated subagent investigation cache daemon (axum + Postgres) that serves prior findings while their read files are unchanged. | `services.subagent-cache.enable` (`modules/services/subagent-cache/default.nix:33`) |
| symphony | Minimal opinionated systemd unit for the Symphony runtime, reading secrets from an EnvironmentFile you control. | `services.symphony.enable` (`modules/services/symphony/default.nix:27`) |
| velocity | Velocity Minecraft proxy. | `services.velocity.enable` (`modules/services/velocity/default.nix:239`) |

Each module exposes more than `enable`. Read the module's `options` block for
the full set (ports, data dirs, tuning, secrets wiring) before relying on
defaults.

## How to enable one

In your image's NixOS modules, import the service module and set its enable
option to `true`. Services typically declare their own
[`ix.healthChecks`](./health-checks.md) and
[`ix.networking`](./networking.md) port claims, so you usually do not wire ports
or probes by hand: enabling the service is enough.

Concrete example, enabling PostgreSQL (option verified at
`modules/services/postgresql/default.nix:21`):

```nix
{
  imports = [ ../modules/services/postgresql ];

  services.ix-postgresql.enable = true;
}
```

Then build the image and roll it out as usual. See [images.md](./images.md) for
how services are composed into an image and [fleet.md](./fleet.md) for deploying
that image across the fleet.

## See also

- [networking.md](./networking.md): port claims services declare via `ix.networking`.
- [health-checks.md](./health-checks.md): health probes services declare via `ix.healthChecks`.
- [images.md](./images.md): composing services into an ix image.
- [fleet.md](./fleet.md): deploying images across the fleet.
