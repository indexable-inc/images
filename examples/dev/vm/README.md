# ix dev VM

What does the VM behind `ix init` look like? This is the forkable ix
environment from RFC 0007: [`dev.nix`](dev.nix) is the ordinary NixOS module
you edit (packages, services, agent toggles), and
[`default.ix`](default.ix) hands it to `index.lib.mkDev`, which layers the
dev base on top: Claude Code and Codex on PATH, this source materialized at
`/ix` inside the guest.

## Run

```sh
ix apply .#dev
```

Run that from a copy of this directory (or the directory `ix init`
scaffolds). Re-running converges the VM in place; edit
[`dev.nix`](dev.nix), apply again, and the change is live.

## Shape

- [`dev.nix`](dev.nix) is the module a user edits after `ix init`.
  Top-level NixOS config (`environment.systemPackages`,
  `programs.git.enable`) is the environment; `ix.dev.agents.*` toggles the
  agent CLIs, which are installed by default.
- [`default.ix`](default.ix) hands the module to `index.lib.mkDev`, passing
  `src = ./.` (the flake source a copied repo wires as `self`). `mkDev`
  returns the same shape as `index.lib.mkVm`, so the flake exposes
  `ix.default` and `nixosConfigurations.dev` like every other example.

## Recursion

The VM has `/ix` (this source) as a local writable copy. It can edit `/ix`
and bring up another VM from the same module, which is what makes the
environment forkable: the file you edit ships with the machine it defines.
