# lib/dev: dev environment helpers

`lib/dev/` is the option surface and module builders behind `mkDev`
(RFC 0007, see [image/dev](../image/overview.md)). It lets a forked `dev.nix`
read like an ordinary NixOS module: write `environment.systemPackages` at the
top level as usual, and reach for `ix.dev.*` only to describe the agents, fleet
shape, and shared identity volume. These files live in `lib/dev/` (not
`modules/`) so the surface is only in scope where a dev image pulls it in: it
must not add `claude-code` to every image in the repo
(`lib/dev/options.nix:10-11`).

`lib/image/dev.nix` consumes all four files: `options.nix`/`agents.nix` as
modules, and `identity.nix`/`shared-mount.nix` as builders it applies per node
(`lib/image/dev.nix:36-43`).

## options.nix: the `ix.dev.*` surface

Declares the option tree `mkDev` reads to plan the fleet
(`lib/dev/options.nix:23-156`):

- `ix.dev.agents.{claude,codex}` (both default true): which agent CLIs to install.
- `ix.dev.baseImage` (default `development-base`): the `images/dev/` image every
  node builds on.
- `ix.dev.selfSource` (default true): materialize the dev source at `/ix` on
  every node so a VM can bring up more VMs from the same spec.
- `ix.dev.fleet.<node> = { replicas, dependsOn, groups, modules }`: the fleet
  topology, mirroring `mkFleet` nodes; default is a single `dev` node
  (`lib/dev/options.nix:56-96`).
- `ix.dev.shared.*`: the shared SMB identity volume:
  `enable`, `mountPoint` (`/shared`), `claude` (bind `~/.claude`, on by default
  so one `claude login` covers the fleet), `ix` (bind `~/.n`, off by default
  since it hands out VM-creation ability), `excludeNodes`, `server`
  (`file-server`), `group` (`ix-dev-shared`, private east-west), `guestOk`
  (`lib/dev/options.nix:98-155`).

## agents.nix: the agent CLI layer

The single source of truth for "our versions of the agents", imported by both
`images/dev/development-base` and `mkDev` so the wrapped `claude` binary and its
managed-settings policy cannot drift (`lib/dev/agents.nix:1-13`). It imports
`options.nix` and, gated on `ix.dev.agents.*`, installs `pkgs.codex` and a
`claude-code` wrapper that bakes `IS_SANDBOX=1` (the guest VM is the sandbox
Claude's root-user guard asks about, `lib/dev/agents.nix:36-41`). It also writes
`/etc/claude-code/managed-settings.json` (Claude's highest-precedence, read-only
layer) to set `permissions.defaultMode = "bypassPermissions"`,
`skipDangerousModePermissionPrompt`, summarized thinking, and effectively
infinite transcript retention (`lib/dev/agents.nix:86-101`). Leaving
`~/.claude/settings.json` app-owned is why binding it onto the shared volume does
not collide with this managed layer.

## identity.nix: what lives on the volume

Module builders for the bound identity directories and `/ix`
(`lib/dev/identity.nix:30-92`):

- `bindModule { mountPoint, binds }`: bind-mounts each `{ localPath,
  shareSubdir }` (e.g. `/root/.claude`) onto `<mountPoint>/<shareSubdir>` with
  `nofail` + `x-systemd.requires-mounts-for` ordering after the CIFS mount
  (`lib/dev/identity.nix:38-52`). Only `~/.claude` / `~/.n` are shared, never the
  whole `~/.config`.
- `sourceNode` / `sourceServerSeed`: materialize `/ix` for recursion: on the
  volume when one exists (writable, fleet-wide), else a local writable copy
  seeded once from the read-only store source.

## shared-mount.nix: the SMB share

`{ serverModule, clientModule }` builders for the dev-fleet identity volume
(`lib/dev/shared-mount.nix:30-`). One node runs userspace `smbd` (ix guests
share the host `linux-ix` kernel, so the server cannot be in-kernel `ksmbd`) and
exports one share; every other node mounts it over CIFS with `cifs.ko`. The
locking knobs keep a concurrent `~/.claude/.credentials.json` token refresh
honest (`lib/dev/shared-mount.nix:5-10`). `serverModule { shareName, shareDir,
guestOk ? true, subdirs ? [] }` pre-creates the bind-mount targets;
`mkDev` calls these with the elected server node (it does not configure Samba
itself). `guestOk` defaults true so the example fleet comes up with no secrets
plumbing; the share is only reachable on the private east-west group. A
production identity volume should set `guestOk = false` and add a Samba user
(`lib/dev/shared-mount.nix:16-23`).
