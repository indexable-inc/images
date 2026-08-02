# hari's dev VM

What does it take to go from nothing to a VM you can actually work in? This
is the RFC 0007 environment pointed at a real person: [`dev.nix`](dev.nix)
adds the `hari` account, sshd and mosh, and then imports
[`../home.nix`](../home.nix) — the same home-manager module his laptop and
hari-compute-1 consume — so the VM is his environment rather than an
approximation of it.

`examples/dev/vm` is the generic teaching copy of the same shape. This one
lives under `users/` because it imports from `users/`, which an example
cannot do: `paths.examples` is a separate `flake = false` subtree and has no
`users/` in it.

## Boot

```sh
# from this directory
ix apply .#dev
```

Iterating on the in-tree index source rather than the published mirror (run
from the ix repository root so both paths stay inside the uploaded source):

```sh
ix apply \
  --workdir index/users/harivansh-afk/dev \
  --override-input index=index \
  .#dev
```

## Connect

```sh
ix shell dev
```

This path works from every ix client and does not depend on a public IPv4.
The VM still runs sshd and mosh, and `ix ls` prints its public IPv6 address.
On a client with IPv6 connectivity, `mosh hari@<address>` remains available;
mosh sshs in only to start `mosh-server`, then uses UDP so the session survives
roaming between networks and a closed lid. sshd is key-only, root login is
off, and Hari's laptop public key is the only authorized key.

## Secrets

[`default.ix`](default.ix) declares the account secret store keys this
environment expects. Values are never in this repo; store them once:

```sh
ix secret set github_token
```

`github_token` arrives at `/run/secrets/github/token` and is read by a git
credential helper, so it never enters the environment or the nix store. Add
another by naming it in `deployment.secrets` and consuming the path.

This is the ix account secret store, not sops. The repo has no sops-nix or
agenix wiring; `/run/secrets/<name>` is the platform's own mechanism and is
the right one here. Running his sops tree in the VM would mean delivering
the age key as an ix secret first, which is a follow-up, not a prerequisite.

## Cache

`lib/per-system.nix` names this environment's closure as
`packages.x86_64-linux.harivansh-dev-system`, which puts it in
`cachePushRoots`. Merges to main build it once and push it to
`cache.ix.dev`, so `ix apply` substitutes a built system instead of
compiling the neovim, Go and Elixir toolchains per VM.

Only the closure is cached, not the `/ix` copy: `src` is threaded in by
[`flake.nix`](flake.nix) for in-guest recursion and changes with every
commit, so it is deliberately left out of the cache root.
