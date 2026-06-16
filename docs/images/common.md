# Images

`images/` is the set of runnable NixOS systems the repo ships as OCI archives
for ix VMs (`flake.nix:2`, "Pre-built OCI images for ix VMs"; `README.md:67`).
Each directory under `images/<family>/<name>/` is a thin NixOS module that names
itself (`ix.image.name`/`tag`) and turns on the services it wants; the shared
image library evaluates it into a full NixOS system and streams the closure out
as one self-contained OCI tar. There is no runtime image stacking: ix runs one
image (`lib/image/default.nix:92-95`).

Read this page first, then the per-image component page. The OCI packaging
mechanics (the layer planner and the Rust tar/describe tool) are owned by the
[nix-build](../nix-build/oci-image-builder/overview.md) domain; VM lifecycle and
fleet convergence by [vm-fleet](../vm-fleet/common.md). This domain owns the
image definitions only.

## Images in this domain

| image | family | flake output | what it builds |
| --- | --- | --- | --- |
| `images/desktop/remote-desktop` | desktop | `.#remote-desktop` | Xpra browser HTML5 desktop (icewm + xterm + firefox). See [remote-desktop](remote-desktop/overview.md). |
| `images/dev/development-base` | dev | `.#development-base` | default agent dev box: Claude Code + Codex, build toolchain, browser automation. See [development-base](development-base/overview.md). |
| `images/dev/kernel-dev` | dev | `.#kernel-dev` | Linux kernel build box; shallow Linus tree cloned to `/src/linux`. See [kernel-dev](kernel-dev/overview.md). |
| `images/dev/neovim-ci` | dev | `.#neovim-ci` | Neovim upstream CI toolchain (clang-21, lua/luajit, test deps). See [neovim-ci](neovim-ci/overview.md). |
| `images/dev/symphony-codex` | dev | `.#symphony-codex` | disposable Symphony agent runner: tmpfs `/workspace`, room-server ports. See [symphony-codex](symphony-codex/overview.md). |
| `images/games/minecraft` | games | `.#minecraft`, `.#minecraft_<ver>` | Java Minecraft server; per-version variants via `versions.nix`. See [minecraft](minecraft/overview.md). |
| `images/games/minecraft-bedrock` | games | `.#minecraft-bedrock` | Bedrock Dedicated Server (native Linux). See [minecraft-bedrock](minecraft-bedrock/overview.md). |
| `images/games/minecraft-status` | games | `.#minecraft-status` | minimal Fabric server used as the ix status/lifecycle canary. See [minecraft-status](minecraft-status/overview.md). |
| `images/games/minestom` | games | `.#minestom` | Minestom hello-world fat-jar server. See [minestom](minestom/overview.md). |
| `images/system/test-cluster-bootstrap` | system | `.#test-cluster-bootstrap` | bare NixOS bootstrap image used to materialize missing fleet nodes. See [test-cluster-bootstrap](test-cluster-bootstrap/overview.md). |

These are Nix-only packages (no Rust crate). They are not enumerated in the root
`Cargo.toml`; they are discovered from the tree (below).

## The OCI-from-NixOS-closure model

An image is built by `ix.mkImage` (`lib/image/default.nix:90-97`):
`mkImage args = (evalImageConfig args).ix.build.ociImage`. `evalImageConfig`
(`lib/image/default.nix:59-88`) runs `lib.nixosSystem` over, in order:

1. one shared `nixpkgs.pkgs` instance (`imagePkgs`, `lib/image/default.nix:39-47`)
   so evaluating many images in one eval does not reinstantiate nixpkgs per node;
   unfree packages enter only by name in this predicate (`yourkit-java`,
   `claude-code`), never by flipping `allowUnfree`.
2. `./platform.nix` - the baseline applied to every image (below).
3. `./oci-layer.nix` - the OCI packaging and `ix.image.{name,tag}` options.
4. Home Manager as a NixOS module (root's per-tool XDG config).
5. `moduleList` - the entire `modules/` registry, added unconditionally.
6. the caller's `modules` (the image's own `default.nix` plus any version overlay).

`oci-layer.nix:64-119` does the packaging. It builds a `systemRoot` FHS layer
(`/init`, `/bin`, `/etc`, `/usr`, ... pointing into `config.system.build.toplevel`),
runs the closure through `pkgs.dockerTools.streamLayeredImage` with
`maxLayers = 67` and `config.Entrypoint = ["${toplevel}/init"]`
(`oci-layer.nix:84-91`), then pipes `streamLayeredImage`'s `passthru.conf` layer
plan into `oci-image-builder` to produce the final `<name>-oci.tar`
(`oci-layer.nix:109-119`). Splitting the closure into ~67 layers lets a registry
dedupe shared store paths across images. The build fails if the layers waste more
than the configured payload budget (`ix.build.ociEfficiency.*`,
`oci-layer.nix:32-58`, default `minEfficiency = 0.95`, `maxWastedBytes = 20 MiB`).

## How an image composes modules

The whole `modules/` registry is in scope for every image:
`moduleList = lib.collect builtins.isPath nixosModules` and
`nixosModules = discoverModules { root = paths.modules }`
(`lib/default.nix:78,94`). Each module stays inert until its `enable` flag is
set, so an image definition is small: set `ix.image.name`/`tag`, flip the
`services.*.enable` it needs, and add `environment.systemPackages`. The Minecraft
loader sub-modules (fabric, paper, ...) are present in every image and gated on
`services.minecraft.<loader>.enable` the same way. The base runtime profile is
auto-enabled (`ix.profiles.base.enable = lib.mkDefault true`,
`oci-layer.nix:62`), so every image already carries the operator toolchain.

## Cross-image invariants (`lib/image/platform.nix`)

Every image inherits these from `platform.nix` and the base profile, so a
component page can assume them:

- Container boot: `boot.isContainer = true`, `/tmp` on tmpfs
  (`platform.nix:401-417`). ix VMs share the host `linux-ix` kernel.
- Networking: `useDHCP = false` (ix provisions the guest address;
  `platform.nix:438`), nftables firewall on (`platform.nix:453-464`). The base
  always opens TCP 5001 (`ix-console`) and UDP 8443 (`ix-agent`)
  (`platform.nix:350-364,456-463`).
- One port/firewall source of truth: `ix.networking.expose.<name>` registers a
  port claim, opens the in-guest firewall, and makes the listener discoverable
  across the fleet (`platform.nix:194-247,292-313`). `ix.networking.portClaims`
  is the lower-level primitive it desugars to; same-namespace collisions fail at
  eval (`platform.nix:366-375`).
- `ix.healthChecks.<name>` declares readiness probes (`unit` sugar for
  `systemctl is-active`, or a `command` run `from = "guest"`/`"host"`) consumed
  by fleet `health`/`up`/`replace` (`platform.nix:264-277,20-118`).
- Operator shell is Nushell (`users.defaultUserShell`, `platform.nix:431`);
  `nix-ld` on so prebuilt binaries find an FHS linker (`platform.nix:424`);
  systemd coredumps captured (`platform.nix:503-510`). The base profile adds
  git/neovim/btop/gdb/strace/ripgrep/fd and a `/work/ix` login workspace
  (`modules/profiles/base/default.nix`).

## How images are discovered and built

`ix.discoverImages { root = paths.images }` walks `images/<family>/<name>/`
and exposes every directory whose path is exactly two segments deep as a flake
package keyed by the directory name (`lib/discovery.nix:124-166`, asserts
`images/<category>/<name>/default.nix` at `:144-146`). The result is merged into
`packages.<system>` (`lib/per-system.nix:693-696,1043-1044`), so:

```
nix build .#<image>     # realize one image OCI tar, e.g. nix build .#minecraft
nix flake show          # list every image package
```

A directory with a sibling `versions.nix` is special: each version key becomes
`<name>_<ver>` and the `default` key gets the unsuffixed `<name>` alias
(`lib/discovery.nix:81-111`). Only `minecraft` uses this
([minecraft](minecraft/overview.md)). Every image also gets an `image-<name>`
build check in `ciChecks` (`lib/per-system.nix:1018-1027`) and an attached
`passthru.tests.eval` evaluation test when `tests/imageTests` has a matching key
(`lib/discovery.nix:152-164`, `tests/default.nix:5063`).

The four families are organizational only (they map to the
`images/<family>/<name>` layout; the package name drops the family):
`desktop`, `dev`, `games`, `system`.

## Glossary

- OCI archive: the output tar of `mkImage`, an OCI image streamed from a NixOS
  closure (`oci-layer.nix:64-119`). Push it with `ix image push` or run it as a
  VM.
- NixOS closure / toplevel: `config.system.build.toplevel`, the activated system
  the OCI `Entrypoint` boots via `/init` (`oci-layer.nix:73,90`).
- base profile: `modules/profiles/base`, auto-enabled cross-cutting operator
  toolchain and shell config (`oci-layer.nix:62`).
- moduleList: the flattened `modules/` registry added to every image; inert
  until an `enable` is flipped (`lib/default.nix:94`).
- loader: a Minecraft server-jar provider module (fabric/paper/folia/...) merged
  into `services.minecraft` and gated on `services.minecraft.<loader>.enable`
  (`modules/services/minecraft/default.nix:1-7`).
- modCatalog: the slug-to-locked-jar map a Minecraft image installs, defaulted
  from `ix.artifacts.minecraft.modCatalogs.<version>` plus `common`
  (`modules/services/minecraft/default.nix:858-872`).
- expose / portClaim: the declarative listener registry; `expose` opens the
  firewall and adds cross-node discovery, `portClaim` is the eval-time collision
  registry (`platform.nix:194-247,280-313`).
- healthCheck: a declared readiness probe surfaced to fleet tooling
  (`platform.nix:264-277`).
- versions.nix: per-image version overlay sidecar; produces `<name>_<ver>`
  packages plus a `<name>` default alias (`lib/discovery.nix:81-111`).

## Components

| component | page | what |
| --- | --- | --- |
| remote-desktop | [remote-desktop/overview.md](remote-desktop/overview.md) | Xpra HTML5 browser desktop |
| development-base | [development-base/overview.md](development-base/overview.md) | default agent dev box (Claude Code + Codex + toolchain) |
| kernel-dev | [kernel-dev/overview.md](kernel-dev/overview.md) | Linux kernel build box with first-boot git-clone |
| neovim-ci | [neovim-ci/overview.md](neovim-ci/overview.md) | Neovim upstream CI toolchain |
| symphony-codex | [symphony-codex/overview.md](symphony-codex/overview.md) | disposable Symphony agent runner |
| minecraft | [minecraft/overview.md](minecraft/overview.md) | Java Minecraft server + per-version variants |
| minecraft-bedrock | [minecraft-bedrock/overview.md](minecraft-bedrock/overview.md) | Bedrock Dedicated Server |
| minecraft-status | [minecraft-status/overview.md](minecraft-status/overview.md) | status/lifecycle canary server |
| minestom | [minestom/overview.md](minestom/overview.md) | Minestom hello-world fat-jar server |
| test-cluster-bootstrap | [test-cluster-bootstrap/overview.md](test-cluster-bootstrap/overview.md) | NixOS bootstrap image for fleet node creation |
