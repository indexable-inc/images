# development-base

`images/dev/development-base` is the default ix dev box: the wrapped agent CLIs
(Claude Code + Codex) plus a normal build toolchain on top of the auto-enabled
base profile. Flake output `.#development-base`. It and `index.lib.mkDev` share
one agent module so a dev fleet and this image cannot drift.

## What it builds

`images/dev/development-base/default.nix` (50 lines):

- `ix.image.name = "development-base"` (`:17`).
- imports the agent CLI layer: `imports = [ ../../../lib/dev/agents.nix ]`
  (`:15`). That module ships both agents by default (`ix.dev.agents.{claude,codex}`).
- adds the build/runtime toolchain to `environment.systemPackages`
  (`:25-49`): `agent-browser` + `chromium` (local browser automation, no cloud
  provider), `cmake`, `gcc`, `gnumake`, `ninja`, `pkg-config`, `rustup`,
  `nodejs`, `python3`.

The base profile already supplies version control, neovim, the Nushell `/work/ix`
workspace, gdb/lldb, strace, tcpdump, jq, btop, bpftrace, and the tar/gzip/zstd
trio needed to stay `ix up`-switchable (`default.nix:1-9`).

## Composed module: `lib/dev/agents.nix`

The single source of truth for "our versions of the agents" (`lib/dev/agents.nix:1-13`).

- `ix.dev.agents.codex` (default true): adds `pkgs.codex` (Apache-2.0; no unfree
  exception; authenticates at first use in the VM) (`:47-51`).
- `ix.dev.agents.claude` (default true): adds a wrapped `claude` binary built
  with `makeWrapper ... --set IS_SANDBOX 1` (`:36-41,53-56`). The wrapper is how
  Claude Code's bypass-permissions mode is accepted while running as root inside
  the guest, without a global `environment.variables`.
- Claude policy is enforced through `/etc/claude-code/managed-settings.json`
  (read-only, highest precedence): `permissions.defaultMode = "bypassPermissions"`,
  `skipDangerousModePermissionPrompt = true`, summarized thinking forced back on,
  and a ~forever transcript retention (`cleanupPeriodDays = 999999`)
  (`:86-101`). `~/.claude/settings.json` stays app-owned.

`pkgs.claude-code` is unfree; the allow-by-name exception lives on the shared
image nixpkgs instance (`lib/image/default.nix:41-46`), not in this image. Setting
a per-image `nixpkgs.config` is ignored and in fact fails an assertion
(`development-base/default.nix:19-23`).

## Build

```
nix build .#development-base
```

## Eval test (`tests/default.nix:3319-3351`)

The attached eval test asserts the image ships `claude-code` and `codex`, does
NOT enable `allowUnfree` globally, keeps unrelated unfree CLIs (e.g.
`cursor-cli`) out, and enforces root's Claude bypass via the parsed
`managed-settings.json` (`defaultMode == "bypassPermissions"` and
`skipDangerousModePermissionPrompt`). These pin the one-exception-by-name
discipline so a refactor cannot silently widen it.

## Notes

- This is the closest thing to a "default" interactive ix dev VM. For a
  fork-and-deploy dev fleet built on the same agent module, see `index.lib.mkDev`
  (`lib/image/dev.nix`), owned by [vm-fleet](../../vm-fleet/common.md).
- Browser automation is local-only (`agent-browser` drives the bundled
  `chromium`), so sandboxes work offline (`default.nix:27-33`).
