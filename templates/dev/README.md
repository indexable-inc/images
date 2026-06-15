# My ix dev environment

A forkable dev VM config (RFC 0007). One [`dev.nix`](dev.nix) is the source of
truth for your VM environment, an optional fleet, and an optional shared SMB
volume that gives a fleet one Claude (and ix) login.

## Start

```sh
nix flake init -t github:indexable-inc/index#dev
```

Then edit [`dev.nix`](dev.nix): add packages, dotfiles, and (optionally) a
`fleet` and a `shared` volume. Commit it to your own repo and fork it freely.

## Use

```sh
# Bring up the fleet (or the single `dev` VM if no fleet is declared):
nix run .#up

# Mirror the other fleet verbs:
nix run .#health
nix run .#diff
nix run .#down
```

`development-base` ships our wrapped `claude-code` and `codex`, so the agents
are present from a plain `inputs.index` import.

## Shared login

Set `shared.enable = true` with `claudeAuth = true` and the fleet shares one
`~/.claude`: the first `claude login` on any node logs in the whole fleet, and
a new replica needs no extra auth. `ixAuth = true` additionally shares `~/.n`
so a node can spin up more VMs from `/ix`.

> Default for new VMs: pointing a bare `ix up` at this config (`ix dev use`)
> is wired in the `ix` CLI; see RFC 0007. Until then, use `nix run .#up`.
