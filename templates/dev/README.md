# My ix dev environment

A forkable dev VM config (RFC 0007). [`dev.nix`](dev.nix) is an ordinary NixOS
module that is the source of truth for your VM environment, an optional fleet,
and an optional shared SMB volume that gives a fleet one Claude (and ix) login.

## Start

```sh
nix flake init -t github:indexable-inc/index#dev
```

Then edit [`dev.nix`](dev.nix): write your environment at the top level
(`environment.systemPackages`, `programs.*`, `services.*`), and use `ix.dev.*`
for the agents, a `fleet`, and a `shared` volume. Commit it to your own repo and
fork it freely. `flake.nix` is boilerplate you should not need to touch.

## Use

```sh
# Bring up the fleet (or the single `dev` VM if no fleet is declared):
nix run .#up

# Mirror the other fleet verbs:
nix run .#health
nix run .#diff
nix run .#down
```

Claude Code and Codex are installed by default (via `development-base`), so the
agents are present from a plain `inputs.index` import. Turn one off with
`ix.dev.agents.codex = false;`.

## Shared login

Set `ix.dev.shared.enable = true` and the fleet shares one `~/.claude`: the
first `claude login` on any node logs in the whole fleet, and a new replica
needs no extra auth. Add `ix.dev.shared.ix = true` to also share `~/.n` so a
node can spin up more VMs from `/ix`.

> Default for new VMs: pointing a bare `ix up` at this config (`ix dev use`) is
> wired in the `ix` CLI; see RFC 0007. Until then, use `nix run .#up`.
