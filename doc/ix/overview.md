# ix platform

`ix` is a platform for running your own VMs: boot an image and you get a VM
you own, with networking, secrets, logs, snapshots, a shell, and declarative
config converged by name. You drive it with the `ix` CLI (`ix new`, `ix up`,
`ix ls`, ...) over a hosted control plane. This page is the map for agents and
people: what is open vs hosted, how to get a first VM, and which page answers
each "how do I do X".

## Open source vs the hosted platform

Two repos, one boundary:

- **`index` (this repo, MIT).** The open tooling: the NixOS library that builds
  VM images and fleet plans (`lib/`), the ready-made service modules
  (`modules/services/`), the base images (`images/`), and a large set of
  standalone tools (search, mcp, tui, dashboard, `ix-fleet`, ...) under
  `packages/`. Shared Nix primitives live here too: checked script writers,
  generic check constructors, PostgreSQL packages such as `postgresql_18_ix`,
  and reusable `bw://folder/item/field` reference grammar. You can read, fork,
  and run all of it (`LICENSE`).
- **The hosted platform (private, proprietary).** The `ix` CLI, the control plane
  it talks to, and the image registry (`registry.ix.dev`) are the hosted product,
  under the Indexable SDK License. The CLI builds your open `index`-based config
  and provisions it on platform infrastructure.

Rule of thumb: if it builds an image, defines a service, or is a tool you run
locally, it is open and lives here. If it creates, runs, or bills a VM, that is
the hosted platform behind the `ix` CLI. `ix-fleet` is open (it renders plans),
but it reaches the VMs through the hosted control plane.

## Get a first VM

```
curl -fsSL https://ix.dev/install.sh | sh   # install the ix CLI
ix login                                     # sign in through the website
ix run ix/base:latest -- echo hello          # boot a VM, run one command
ix ls                                        # see it; address it by name
```

`ix run` boots a fresh VM, runs the command, and leaves the VM up. For a config
you own and re-converge instead of a one-off, scaffold and bring it up
declaratively:

```
ix init          # write a minimal flake.nix + ix.nix
ix up            # build your NixOS config and converge the VM in place
```

`ix up` is the declarative path: re-running it reconverges, the same contract as
`nixos-rebuild switch`. See [lifecycle.md](lifecycle.md) for when to use which.

## The map

Start here, then follow the page that matches the question:

| Page | Answers |
| --- | --- |
| [cli.md](cli.md) | The `ix` verb map and mental model. For flags, run `ix <verb> --help`. |
| [lifecycle.md](lifecycle.md) | "I want to X" -> command: create, run, converge, stop, snapshot, destroy. |
| [fleet.md](fleet.md) | Declarative multi-VM fleets via the separate `ix-fleet` tool. |
| [images.md](images.md) | Build an image, tag/push to `registry.ix.dev`, boot a VM from it. |
| [networking.md](networking.md) | Expose ports, private VM-to-VM groups, `<host>.ix.internal`, public ingress. |
| [secrets.md](secrets.md) | Declare a secret, attach it to a VM, materialize it in the guest. |
| [health-checks.md](health-checks.md) | Write checks; `from: guest` vs `from: host`; the `unit:` shortcut. |
| [services.md](services.md) | The ready-made services in `modules/services/` and how to enable one. |
| [environment.md](environment.md) | The user-facing `IX_*` and credential environment variables. |
| [glossary.md](glossary.md) | Disambiguate overloaded names (search, run, mcp, fleet, dashboard, index). |

For the package-level "from source" reference behind these pages, see the
per-package docs under [`doc/`](../index.md), for example the
[ix-fleet overview](../ix-fleet/overview.md).

## See also

- [cli.md](cli.md): the `ix` verb map and mental model.
- [lifecycle.md](lifecycle.md): which command for create, run, converge, snapshot, destroy.
- [fleet.md](fleet.md): declarative multi-VM fleets via `ix-fleet`.
- [glossary.md](glossary.md): disambiguating overloaded names.
