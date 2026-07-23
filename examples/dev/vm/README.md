# ix dev VM

How do you get a forkable agent workstation out of one NixOS module? This is
the ix environment from RFC 0007: [`dev.nix`](dev.nix) is the ordinary
module you edit — your packages and services at the top level, `ix.dev.*`
only for the agent tooling — and [`default.ix`](default.ix) hands it to
`index.lib.mkDev`, which layers the dev base (wrapped `claude-code` and
`codex` CLIs, via `lib/dev/agents.nix`) underneath and yields one VM named
`dev`.

## Run

```sh
# From this directory (examples/dev/vm in the index repo), or a copy of it.
ix apply .#dev
```

Need the repo first? `git clone https://github.com/indexable-inc/index`.

## Shape

- [`dev.nix`](dev.nix) is the module a user edits after `ix init`. Top-level
  NixOS config (`environment.systemPackages`, `programs.git.enable`) is the
  environment; `ix.dev.agents` toggles the installed agents. Claude Code and
  Codex are installed by default.
- [`default.ix`](default.ix) hands the module to `index.lib.mkDev`, passing
  `src` (the flake's `self`) so the source lands at `/ix` on the VM.

## Recursion

The VM has `/ix` (this source) as a local writable copy. It can edit `/ix`
and bring up more VMs from the same module. (Shipping the `ix` CLI on `PATH`
inside the guest is the cross-repo follow-up RFC 0007 notes; this example
places the source.)
