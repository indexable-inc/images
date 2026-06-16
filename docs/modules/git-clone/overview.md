# git-clone

`modules/services/git-clone/default.nix` clones a git repository on first boot
and does nothing on later boots. The clone is idempotent: a boot that finds
`<dest>/.git` already present skips the fetch. It uses `gitoxide` (`gix`), not C
git.

Option namespace: `services.git-clone` (`default.nix:20`).

## Public surface (options)

- `enable` (`default.nix:21`).
- `url` (str, required) - repository to clone (`default.nix:23`).
- `dest` (str, default `/repo`) - clone target (`default.nix:25`).
- `shallow` (bool, default true) - clone with `--depth 1` (`default.nix:30`).
- `ref` (nullable str, default null) - branch/tag/ref to check out; renders
  `--ref` when set (`default.nix:35`).
- `activation` (enum `multi-user` | `timer`, default `multi-user`) - how the
  clone is started (`default.nix:40`). Use `timer` for large repositories that
  should be fetched after boot readiness rather than blocking
  `multi-user.target`.

## What it produces

- `environment.systemPackages = [ pkgs.gitoxide ]` (`default.nix:54`).
- `systemd.services.git-clone` (`default.nix:56`): a `Type = oneshot`,
  `RemainAfterExit = true` unit after `network-online.target`. Its `path`
  carries `coreutils` and `gitoxide`; the script guards on
  `[ ! -d "<dest>/.git" ]`, makes the parent dir, then runs
  `gix clone <depthFlag> <refFlag> <url> <dest>` (`default.nix:69-80`). The unit
  is wanted by `multi-user.target` only when `activation == "multi-user"`.
- `systemd.timers.git-clone` (`default.nix:83`): present only when
  `activation == "timer"`; `OnBootSec = 15s`, fires `git-clone.service`.

No port claim, no health check, no `ix` argument.

`TODO` in source: use cross-VM shared CAS to speed up clones (`default.nix:3`).

## How it is wired

Auto-discovered as `services/git-clone`. No flake output; pure NixOS module.
