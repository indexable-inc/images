# My ix environment

A forkable ix VM config (RFC 0007). [`ix.nix`](ix.nix) is an ordinary NixOS
module that is the source of truth for your VM environment, an optional fleet,
and an optional shared SMB volume that gives a fleet one Claude (and ix) login.

## Start

```sh
nix flake init -t github:indexable-inc/index#ix
```

Then edit [`ix.nix`](ix.nix): write your environment at the top level
(`environment.systemPackages`, `programs.*`, `services.*`), and use `ix.dev.*`
for the agents, a `fleet`, and a `shared` volume. Commit it to your own repo and
fork it freely. `flake.nix` is boilerplate you should not need to touch.

## Use

Out of the box (no `ix.dev.fleet` declared) this config is a **single VM named
`dev`**. One command builds `ix.nix` into an OCI image and creates or updates that VM:

```sh
nix run .#up
```

That is the "consume my `ix.nix` for a new VM" path: `nix run .#up` realises
the image from your config and creates or updates the VM through `ix up`. Re-run it after editing `ix.nix` to roll the VM forward.

Declare nodes under `ix.dev.fleet` and the same command brings up the whole
fleet instead. The other verbs mirror `ix fleet <sub>`:

```sh
nix run .#health
nix run .#diff
nix run .#down
```

Claude Code and Codex are installed by default (via the dev base module), so the
agents are present from a plain `inputs.index` import. Turn one off with
`ix.dev.agents.codex = false;`.

## Shared login

Set `ix.dev.shared.enable = true` and the fleet shares one `~/.claude`: the
first `claude login` on any node logs in the whole fleet, and a new replica
needs no extra auth. Add `ix.dev.shared.ix = true` to also share `~/.n` so a
node can spin up more VMs from `/ix`.

> Default VM path: `ix up` should discover `./ix.nix`; until that CLI path lands, use `nix run .#up`.
