# kernel-dev

`images/dev/kernel-dev` is a Linux kernel build box: a C build toolchain plus a
shallow clone of Linus' tree at `/src/linux`, fetched after boot so it does not
block startup. Flake output `.#kernel-dev`.

## What it builds

`images/dev/kernel-dev/default.nix` (20 lines):

- `ix.image.name = "linux-kernel-dev"` (`:5`). Note the OCI image name differs
  from the flake output name (`kernel-dev`).
- adds `gnumake`, `gcc`, `gnugrep`, `findutils` to `environment.systemPackages`
  (`:7-12`). The base profile already brings ripgrep, fd, neovim, gdb, and perf
  (`default.nix:1-2`).
- clones the kernel on first boot via a timer:

```nix
services.git-clone = {
  enable = true;
  activation = "timer";                         # default.nix:14-19
  url = "https://github.com/torvalds/linux.git";
  dest = "/src/linux";
};
```

## Composed module: `services.git-clone`

Defined in `modules/services/git-clone/default.nix`. Clones a repo once and is
idempotent: a later boot sees `.git` present and does nothing (`:1-2,76-79`).

- `enable` (`:21`), `url` (`:23`), `dest` (default `/repo`, `:25-28`),
  `shallow` (default true, renders `--depth 1`, `:30-33,71`), `ref`
  (default null, `:35-38`).
- `activation` enum `multi-user` | `timer` (default `multi-user`, `:40-50`).
  `timer` is for large repos: the clone runs after boot readiness instead of
  blocking `multi-user.target`. kernel-dev sets `timer` because the Linux tree is
  large.
- Runtime: ships `pkgs.gitoxide` and runs `gix clone <flags> <url> <dest>` in a
  oneshot `git-clone.service` after `network-online.target`
  (`:53-81`); with `activation = "timer"` the service is not `wantedBy`
  `multi-user.target` and is instead started by a `git-clone.timer`
  (`OnBootSec = 15s`, `wantedBy = timers.target`, `:83-90`).

## Build

```
nix build .#kernel-dev
```

After boot wait ~15s, then build in `/src/linux` with the standard kernel
toolchain. The clone is shallow (`--depth 1`); pass `services.git-clone.shallow =
false` in a fork if you need history.

## Eval test (`tests/default.nix:3304-3317`)

Asserts git-clone is enabled, and that the timer activation does NOT make the
clone service `wantedBy = multi-user.target` while the timer IS `wantedBy =
timers.target` (`kernelDev.git.clone.service.wantedBy == []`,
`...timer.wantedBy == ["timers.target"]`). This pins the "do not block boot on a
large clone" behavior.
