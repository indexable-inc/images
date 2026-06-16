# lib/image: OCI image and fleet builders

`lib/image/` turns NixOS module sets into OCI archives, evaluates Colmena-style
fleets of those images, and adds the opinionated dev-fleet layer. It also owns
the base platform module every image inherits. `lib/default.nix:445-468` imports
this directory and re-exports `evalImageConfig`, `mkImage`, `mkNonNixImage`,
`mkFleet`/`mkFleetFor`, and `mkDev`/`mkDevFor`. The OCI tar is produced by the
[oci-image-builder](../../nix-build/oci-image-builder/overview.md) Rust CLI;
this directory is the Nix front end.

## Files

| file | role |
| --- | --- |
| `lib/image/default.nix` | the builder front ends; one shared `imagePkgs` instance |
| `lib/image/platform.nix` | base platform module: `ix.healthChecks`, `ix.networking.*`, boot/firewall/journald defaults |
| `lib/image/oci-layer.nix` | `ix.image.*` options + the `streamLayeredImage` -> OCI tar build |
| `lib/image/non-nix-oci.nix` | `mkNonNixImage`: OCI on a pinned non-Nix base |
| `lib/image/fleet.nix` | `mkFleet`: fleet spec -> plan + per-node images + CLI wrappers |
| `lib/image/dev.nix` | `mkDev`: dev-fleet layer over `mkFleet` (RFC 0007) |
| `lib/image/health-checks.nix` | the `health-checks` apps that boot every example fleet and verify it |

## evalImageConfig and mkImage

`evalImageConfig { modules ? [] }` (`lib/image/default.nix:59-88`) is the one
evaluation path every image build and eval test uses. It runs `lib.nixosSystem`
with `specialArgs.ix = ixSpecialArgs` (`lib/image/default.nix:65`) over
`platform.nix`, `oci-layer.nix`, the home-manager NixOS module, the full
auto-discovered `moduleList`, and the caller's `modules`, returning the evaluated
`config`. Every image shares ONE `imagePkgs` nixpkgs instance (via
`nixpkgs.pkgs`, `lib/image/default.nix:39-47`, `66-67`) instead of
re-instantiating per node, which is what keeps multi-image evals from paying
nixpkgs instantiation 20-30 times over.

`mkImage args` (`lib/image/default.nix:90-97`) is `(evalImageConfig
args).ix.build.ociImage`. Each image is self-contained; ix runs one, it does not
stack images at runtime. The result is an OCI-archive derivation usable as a
`packages.<system>.<name>` output or pushed with `ix image push`.

Unfree packages enter images only by explicit name in the
`allowUnfreePredicate` on `imagePkgs` (`yourkit-java`, `claude-code`,
`lib/image/default.nix:41-46`), never by flipping `allowUnfree`.

## oci-layer.nix: the OCI build

Declares `ix.image.{name,tag}`, `ix.build.ociImage` (internal), and
`ix.build.ociEfficiency.*` (`lib/image/oci-layer.nix:16-58`). The build splits
the NixOS toplevel closure into `maxLayers = 67` OCI layers via
`dockerTools.streamLayeredImage` (under the 127-layer registry cap), adds a
`systemRoot` FHS layer (`/init`, `/bin`, `/etc`, ...,
`lib/image/oci-layer.nix:71-82`), then runs `oci-image-builder` to stream the
final tar (`lib/image/oci-layer.nix:108-119`). The efficiency gate fails the
build when wasted layer payload crosses `minEfficiency` (0.95),
`maxWastedBytes` (20 MiB), or `maxWastedPercent` (0.20).

## platform.nix: the base every image inherits

The baseline module merged into every image (`lib/image/platform.nix:1`). Its
option surface is the cross-image contract:

- `ix.healthChecks.<name>` (`lib/image/platform.nix:264-277`): commands that
  prove a service is ready. Each is `{ command | unit, from = "guest"|"host",
  timeoutSec, attempts, intervalSec, requiresIpv4 }`. Setting `unit = "nginx"`
  desugars to a `systemctl is-active` probe (`lib/image/platform.nix:114-116`);
  `from = "host"` runs the probe on the operator host with `IX_NODE*` env vars
  for public-reachability checks. Fleet `up`/`replace`/`switch` wait on these.
- `ix.networking.expose.<name>` (`lib/image/platform.nix:292-313`): the one
  declaration for "this image listens here". It registers a port claim (so
  same-namespace collisions throw at eval time,
  `lib/image/platform.nix:366-375`), opens the in-guest firewall unless
  `firewall = false`, and makes the listener discoverable cross-node via
  `ix.endpointOf` (see [util/endpoint](../util/overview.md)).
- `ix.networking.portClaims.<name>` (`lib/image/platform.nix:280-290`): the
  lower-level claim registry `expose` desugars to; `ix.networking.groups` and
  `eastWest.hostName` are the east-west membership primitives.

It also sets the platform defaults: `boot.isContainer`, tmpfs `/tmp`,
`nftables` firewall opening the exposed ports, Nushell as default shell,
bounded journald, `userdbd`, coredump capture, and modern nix features
(`lib/image/platform.nix:401-510`).

## non-nix-oci.nix: mkNonNixImage

`mkNonNixImage { name, baseImage, tag?, contents?, config?, maxLayers?, efficiency? }`
(`lib/image/non-nix-oci.nix:52-181`) builds an OCI image on a pinned non-Nix
base (ubuntu, debian, distroless): no NixOS, no systemd PID 1; the base
userland is the rootfs and `contents` are extra Nix store layers on top. The
base is pulled by digest (`dockerTools.pullImage`). The build is sharded into a
content-addressed `image.json` description (one derivation per store layer, so a
one-path change re-tars one layer) and a `materialize` step that regenerates the
tar and enforces the efficiency gate (`lib/image/non-nix-oci.nix:91-181`). The
description is on `passthru.description`.

## fleet.nix: mkFleet

Curried `mkFleetFor hostSystem` (`lib/image/default.nix:112-129`), then a fleet
spec `{ defaults ? [], deployment ? {}, secrets ? {}, nodes }`
(`lib/image/fleet.nix:19-24`). Each node is a NixOS module set (or a wrapped
`{ modules, deployment, tags, groups, dependsOn, replicas }` node,
`lib/image/fleet.nix:94-104`) built through `evalImageConfig`. The `deployment`
attrset is typo-checked against the known keys (`lib/image/fleet.nix:66-92`):
`bootstrapImage`, `region`, `ipv4`, `snapshot`, `switch`, `env`, `l7ProxyPorts`,
`recreateOnUp`, `destination`. `secrets` is normalized via the
[secrets](../util/overview.md) helper.

The result (`lib/image/fleet.nix:377-403`) exposes the rendered `plan`, per-node
`packages` (the OCI images), `systemPackages` (toplevels), `nodes`/`meta`, the
CLI subcommands `up`/`down`/`diff`/`health`/`switch`/`replace`/`bootstrap`
(each a Nushell wrapper over the `ix-fleet` binary with `--plan`,
`lib/image/fleet.nix:368-386`), and `withNodePrefix` for renaming nodes while
sharing the underlying closures.

## dev.nix: mkDev

`mkDevFor hostSystem { module, src ? null }` (`lib/image/dev.nix:49-54`): an
opinionated dev-fleet layer over `mkFleet` (RFC 0007) that consumes one forkable
`dev.nix` module and returns the same result shape `mkFleet` does. It reads
`ix.dev.*` (see [dev](../dev/overview.md)) via a probe eval
(`lib/image/dev.nix:60-66`), then synthesizes: the agent layer + user module as
`mkFleet` `defaults`, `ix.dev.fleet` as nodes, an optional `file-server` SMB node
plus identity binds when `ix.dev.shared.enable`, and `/ix` source materialization
on every node. `templates/dev` is the forkable starting point
(`flake.nix:326-329`).

## health-checks.nix

`health-checks` / `health-checks-zellij` apps (`lib/image/health-checks.nix`)
that boot every example fleet in parallel through `ix-fleet up`, verify the
declared `ix.healthChecks`, and tear the VMs down. The `dag` front end uses
`dag-runner` for headless/CI use; `zellij` gives one pane per fleet for
interactive triage. Wired in `lib/per-system.nix`.
