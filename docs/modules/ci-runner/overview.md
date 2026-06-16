# ci-runner

`modules/services/ci-runner/default.nix` runs a static pool of self-hosted
GitHub Actions runners on a persistent NixOS host. The point is cache locality:
jobs reuse the host's shared `/nix/store` and the `indexable-inc` Cachix
substituter, so `nix build .#...` pulls warm artifacts instead of rebuilding
from a cold store each run. Per-job work directories are still wiped (ephemeral
runners); only the shared store survives. It is the small counterpart to the
repo's webhook dispatcher, with no just-in-time minting and no per-job VM.

Option namespace: `services.ci-runner` (`default.nix:28`).

## Public surface (options)

- `enable` - turn on the runner pool (`default.nix:29`).
- `url` (str) - repository or org URL the runners register against
  (`default.nix:31`).
- `tokenFile` (str, runtime path not `types.path`) - file with a fine-grained
  GitHub PAT with read/write to the repo's self-hosted runners
  (`default.nix:40`). A PAT, not a one-hour registration token, is required
  because ephemeral runners mint a fresh registration on every restart. The
  path is a runtime string so the PAT never enters the world-readable Nix store.
- `count` (positive int, default 2) - runners registered in parallel; each
  processes one job at a time, so this is the host's CI concurrency
  (`default.nix:55`). Runner names are `index-1 .. index-<count>`
  (`default.nix:25`).
- `labels` (list of str, default `[ "nix" ]`) - extra labels appended to every
  runner (`default.nix:64`).
- `ephemeral` (bool, default true) - register single-use runners that
  de-register after one job and re-register on restart (`default.nix:73`).
- `packages` (list of package, default `[]`) - extra packages on each job's
  PATH on top of git, Nix, and Cachix tooling (`default.nix:85`).

## What it produces

`config` (`default.nix:96`) does two things and declares no port claim:

- **Nix daemon settings** (`default.nix:101-117`), all `extra-*` so they add to
  rather than replace host defaults: enables `nix-command`/`flakes`,
  `accept-flake-config = true` (avoids the interactive substituter prompt that
  stalls `nix flake check`), adds the `indexable-inc.cachix.org` substituter and
  trusted key, and advertises `extra-system-features = [ "gccarch-znver5" ]`
  (index images pin `gcc.arch = znver5`, so the daemon must advertise that
  builder feature or refuse the builds).
- **Runner instances** via `services.github-runners` (`default.nix:119`), one
  per generated name with `enable`, `url`, `tokenFile`, `ephemeral`,
  `extraLabels = labels`, `replace = true` (re-register under the same name
  after a host config change instead of failing on a name clash), and
  `extraPackages = [ cachix gh git config.nix.package ] ++ packages`.

## How it is wired

Auto-discovered as `services/ci-runner`. No flake package output of its own; it
configures the upstream nixpkgs `services.github-runners` module. Imports
`config`, `lib`, `pkgs` only (no `ix` argument), so it carries no port claim or
health check.
