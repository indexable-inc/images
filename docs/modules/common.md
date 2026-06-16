# modules

`modules/` is the repo's reusable NixOS configuration: auto-discovered service
modules (`modules/services/`), opt-in runtime profiles (`modules/profiles/`),
and one home-manager module (`modules/home/`). Each is an inert option set that
an image, fleet, or example imports and turns on with an `enable` flag. The
modules are the deploy-time face of the Rust/Nix packages under `packages/`: a
service module wires a built package into a systemd unit, a port, and a health
check on a VM. This page is the orientation; read it before any component page.

Read alongside the build/lib domain ([nix-lib](../nix-lib/common.md)): the
`ix.networking.portClaims` / `ix.healthChecks` / `ix.systemdHardening`
primitives these modules write into are defined there, not here.

## How modules are discovered and wired

There is no module registry to edit. `lib/discovery.nix` walks the tree:

- `discoverModules { root = paths.modules; }` (`lib/default.nix:78`) calls the
  generic `discoverTree` walker (`lib/discovery.nix:20`). A directory becomes a
  module when it has its own `default.nix` (`lib/discovery.nix:184-218`).
- A directory or `.nix` file whose name starts with `_` is skipped with its
  subtree (`lib/discovery.nix:34`), so a module can keep helper data
  (`observability/_dashboards/`, the Nushell/Lua under `profiles/base/`)
  beside its `default.nix` without being treated as a module.
- Sibling directories that each have a `default.nix` nest: `services/minecraft/`
  ships `{ default = ./minecraft; fabric = ./minecraft/fabric; ...; mods = { bluemap = ...; }; }`
  (`lib/discovery.nix:204-216`). A category directory (`services/`, `profiles/`)
  must NOT have its own `default.nix` (`lib/discovery.nix:196-197`).
- The walk returns a nested attrset of paths, exposed as `ix.nixosModules`
  and as the flake's `nixosModules` output (`flake.nix:274`).
- `moduleList = lib.collect builtins.isPath nixosModules` (`lib/default.nix:94`)
  flattens it; `lib/image/default.nix:86` appends `moduleList` to every built
  image unconditionally, so every option is always in scope and each module
  stays inert until its `enable` flag is set (`lib/default.nix:91-94`).

`modules/home/` is NOT swept into `nixosModules`: it contains a bare
`raycast.nix`, not a `default.nix` directory, so discovery yields nothing for it.
It is wired by explicit path as `homeModules.raycast` (`flake.nix:295`). The
other home-manager modules (`portable-services`, `mutable-json`) live outside
`modules/` in `lib/services/` on purpose (`lib/default.nix:80-89`).

## Option-namespace convention

- A service module owns `options.services.<name>` matching its directory name:
  `services.minecraft`, `services.velocity`, `services.geyser`,
  `services.symphony`, `services.ci-runner`, `services.git-clone`,
  `services.remote-desktop`, `services.resource-monitor`, `services.minestom`,
  `services.minecraft-bedrock`, `services.floodgate`.
- A module that wraps or extends an upstream nixpkgs service, or is a cluster
  orchestrator, uses an `ix-` prefix to avoid clobbering the stock option tree:
  `services.ix-postgresql`, `services.ix-seaweedfs`, `services.ix-observability`,
  `services.ix-ray`, `services.ix-spark`.
- Profiles own `options.ix.profiles.<name>` (`base`, `jvm`) or a bare option
  tree (`ix.extendedAttributes`). The home module owns
  `options.programs.raycast.focus`.

## Cross-component invariants

Every service page below assumes these, supplied by the `ix` specialArg and the
platform module (see [nix-lib](../nix-lib/common.md)):

- **Ports are claimed, not hard-coded.** A module registers
  `ix.networking.portClaims.<name>` (one source of truth: protocol, port,
  address) and separately opens `networking.firewall.allowed{TCP,UDP}Ports`,
  gated by a per-module `openFirewall` option. The port-claim registry detects
  same-namespace collisions at eval time (`lib/image/platform.nix`).
- **Readiness is a probe.** Modules register `ix.healthChecks.<name>` with a
  real command (`pg_isready`, `mc-probe`, an HTTP `/healthz`) or `unit` sugar
  (`systemctl is-active`), run from the guest or the operator host.
- **Units are hardened.** A service merges `ix.systemdHardening`
  (`lib/services/systemd-hardening.nix`, surfaced as `ix.systemdHardening`)
  into `serviceConfig`, then overrides only what it must (Ray and Spark turn
  `PrivateDevices`/`PrivateUsers` off for shared memory).
- **`ix` carries the toolbox.** Modules read `ix.artifacts.minecraft.*` (locked
  jars), `ix.packages.*` (`mc-probe`, `mcp`), `ix.languages.java.yourkit`,
  `ix.writeNushellApplication`, `ix.buildSvelteSite`, `ix.cargoUnit`,
  `ix.relativePath`, and `ix.minecraft.*` from this argument.
- **The base profile is automatic.** `lib/image/oci-layer.nix:62` sets
  `ix.profiles.base.enable = lib.mkDefault true`, so every image gets the base
  CLI unless an image opts out.
- **Ray and Spark are repo-agnostic.** They declare no `ix.*` options and take
  the index lib through `_module.args.indexLib` (named, not `ix`, to avoid the
  host's `ix` specialArg), so they import into any NixOS system.

## Services table

| service | option namespace | runs | default ports |
| --- | --- | --- | --- |
| [ci-runner](ci-runner/overview.md) | `services.ci-runner` | nixpkgs `github-runners` pool | none (outbound) |
| [git-clone](git-clone/overview.md) | `services.git-clone` | `gitoxide` (`gix clone`) oneshot | none |
| [postgresql](postgresql/overview.md) | `services.ix-postgresql` | `postgresql_18` tuned for Zen 5 | 5432/tcp |
| [seaweedfs](seaweedfs/overview.md) | `services.ix-seaweedfs` | `weed server -s3` (single node) | 8333/tcp (S3) |
| [observability](observability/overview.md) | `services.ix-observability` | OTel Collector + ClickHouse + Grafana | 4317/4318, 9000/8123, 3000 |
| [resource-monitor](resource-monitor/overview.md) | `services.resource-monitor` | `resource-monitor-stats-writer` crate + Svelte site via nginx | 80/tcp |
| [remote-desktop](remote-desktop/overview.md) | `services.remote-desktop` | Xpra `start-desktop` + icewm | 6080/tcp |
| [symphony](symphony/overview.md) | `services.symphony` | `packages/symphony` (`/bin/symphony`, Phoenix) | 4040/tcp |
| [ray](ray/overview.md) | `services.ix-ray` | `python3Packages.ray` + ix-mcp engine (`packages/mcp`) | 6379/6380/6381, 10001, 10002-10031, 8799 |
| [spark](spark/overview.md) | `services.ix-spark` | `spark-hive` + Gluten/Velox | 7077/8080/8081, 15002, 7078-7080 |
| [minecraft](minecraft/overview.md) | `services.minecraft` | Java server jar (loader-set) via Temurin JRE | 25565/tcp, 25575 rcon |
| [minecraft-bedrock](minecraft-bedrock/overview.md) | `services.minecraft-bedrock` | Bedrock Dedicated Server (fetched) | 19132/19133 udp |
| [minestom](minestom/overview.md) | `services.minestom` | user fat jar via Temurin JRE (ZGC) | 25565/tcp |
| [velocity](velocity/overview.md) | `services.velocity` | Velocity proxy jar (`ix.artifacts`) | 25565/tcp |
| [geyser](geyser/overview.md) | `services.geyser` | Geyser jar as a Velocity plugin | 19132/udp |
| [floodgate](floodgate/overview.md) | `services.floodgate` | Floodgate jar as a Velocity plugin | none (rides Velocity) |

## Profiles and home

- [profiles](profiles/overview.md): `ix.profiles.base` (auto-enabled CLI/shell
  toolbox + BBR + Home Manager root config), `ix.profiles.jvm` (JRE + `JAVA_HOME`),
  `ix.extendedAttributes` (apply `user.*` xattrs at activation).
- [home](home/overview.md): `programs.raycast.focus`, a macOS home-manager module
  writing the `com.raycast.macos` Focus session defaults.

## Glossary

- **port claim**: `ix.networking.portClaims.<name>`, the single declared owner
  of a (namespace, protocol, port). Firewall opening is separate and gated by
  `openFirewall`.
- **health check**: `ix.healthChecks.<name>`, a readiness probe run by
  `nix run .#health-checks`, from `guest` or operator `host`.
- **systemd hardening**: the shared `serviceConfig` baseline merged from
  `ix.systemdHardening` before per-service overrides.
- **loader**: a Minecraft server flavor (fabric, paper, vanilla, ...) that fills
  the `serverJar`/`dropinDir` slots of `services.minecraft` via `mkMinecraftLoader`.
- **dropinDir**: the subdirectory mod/plugin jars are symlinked into; `mods` for
  fabric/neoforge/sponge, `plugins` for paper/folia/purpur/spigot.
- **agent / stack**: the two observability roles; an agent forwards OTLP to a
  stack node that runs ClickHouse + Grafana.
- **fleet**: the Python distributed API (in `packages/mcp`) that `services.ix-ray`
  is the deployment side of.
- **Geyser / Floodgate**: Bedrock-to-Java protocol bridge and its auth bridge,
  both installed as Velocity proxy plugins.
- **EnvironmentFile**: a systemd secrets file path a module reads (symphony) so
  any secret manager can be wired underneath.

## Components

| component | page | what |
| --- | --- | --- |
| ci-runner | [ci-runner/overview.md](ci-runner/overview.md) | self-hosted GitHub Actions runner pool with warm Nix cache |
| git-clone | [git-clone/overview.md](git-clone/overview.md) | idempotent boot-time `gix clone` of a repo |
| postgresql | [postgresql/overview.md](postgresql/overview.md) | PostgreSQL 18 tuned + hugepages, `services.ix-postgresql` |
| seaweedfs | [seaweedfs/overview.md](seaweedfs/overview.md) | single-node S3 object store (`weed server -s3`) |
| observability | [observability/overview.md](observability/overview.md) | OpenTelemetry + ClickHouse + Grafana telemetry stack |
| resource-monitor | [resource-monitor/overview.md](resource-monitor/overview.md) | stats-writer crate + Svelte UI for VM usage/billing |
| remote-desktop | [remote-desktop/overview.md](remote-desktop/overview.md) | browser desktop over Xpra HTML5 |
| symphony | [symphony/overview.md](symphony/overview.md) | Symphony runtime systemd unit + host codex placement |
| ray | [ray/overview.md](ray/overview.md) | tailnet Ray cluster + ix-mcp engine for `fleet` |
| spark | [spark/overview.md](spark/overview.md) | standalone Spark + Gluten/Velox native engine |
| minecraft | [minecraft/overview.md](minecraft/overview.md) | loader-agnostic Java server, mods/plugins/datapacks, loaders + mod submodules |
| minecraft-bedrock | [minecraft-bedrock/overview.md](minecraft-bedrock/overview.md) | Bedrock Dedicated Server |
| minestom | [minestom/overview.md](minestom/overview.md) | Minestom fat-jar runtime |
| velocity | [velocity/overview.md](velocity/overview.md) | Velocity Minecraft proxy + managed plugins/config |
| geyser | [geyser/overview.md](geyser/overview.md) | Bedrock-to-Java bridge as a Velocity plugin |
| floodgate | [floodgate/overview.md](floodgate/overview.md) | Floodgate Bedrock auth bridge as a Velocity plugin |
| profiles | [profiles/overview.md](profiles/overview.md) | base / jvm runtime profiles + extended-attributes |
| home | [home/overview.md](home/overview.md) | raycast Focus home-manager module (macOS) |
