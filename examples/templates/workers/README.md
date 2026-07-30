# VM templates

Adding another worker should not mean editing a file, reviewing a diff, and
copy-pasting an `mkVm` call that can then drift from its siblings. This example
declares a `worker` **template** — a function from params to a VM — and two
instances of it, next to one ordinary named VM:

```sh
ix apply .#web .#worker-1 .#worker-2
```

The model is systemd's, because systemd already solved this split: a unit file
is declarative and versioned, and starting an instance of it is an imperative
act the system then remembers.

| systemd                     | Here                                             |
| --------------------------- | ------------------------------------------------ |
| `foo.service`               | `web`, a named VM in [`default.ix`](default.ix)   |
| `foo@.service` template     | `templates.worker`, a function in the same file  |
| `%i` specifier              | the `instance` param, injected by ix             |
| preset file                 | the `instances` block                            |
| `daemon-reload`             | `ix apply`                                       |
| `systemctl start foo@bar`   | `ix new worker bar` — **not built yet**          |

## What is here and what is not

The `instances` block is the half that works with no server-side state at all:
those instances are declared in the repo, so `ix apply` creates and converges
them exactly like the named VM beside them. That is the whole of this example.

`ix new worker --set port=9000`, which creates an instance imperatively and
records it server-side so every later apply re-renders it from the *current*
template, is the other half of
[RFC 0042](https://ix.dev/plans/0042-vm-templates) and does not exist yet
(ix#9242). Nothing here waits for it.

Two things this version deliberately cannot do:

- **Wire an instance to another VM.** Instances are rendered one layer above
  the config, in [`flake.nix`](flake.nix), so nothing inside `default.ix` can
  hand an instance's `nixosConfigurations` to a peer's `nodes` argument. `web`
  therefore does not proxy to the workers. Named VMs wired to each other work
  today — see [`examples/multi-vm/microservices`](../../multi-vm/microservices).
- **Type its params.** `port` and `shards` are unchecked at the boundary. A
  misspelled param is still caught (nix refuses an unexpected argument by name,
  and the module system types everything a param reaches), just later and
  further from the typo. Annotating them would check them at eval time and
  generate a JSON Schema from the same declaration
  ([index#4450](https://github.com/indexable-inc/index/pull/4450) shipped that
  generator) — but the schema only carries a template's params when they are one
  named parameter, because an alias cannot annotate a destructured pattern and an
  inline annotation never reaches the document. Trading `port = 8080` for
  `params.port or 8080` to buy editor completion is not obviously worth it, so
  this example stays untyped until
  [RFC 0042](https://ix.dev/plans/0042-vm-templates)'s Typed params gap closes.

## Two names for one instance

`worker@1` is the **instance** name: systemd's spelling, what you write in the
`instances` block, and what `ix new` will record. `worker-1` is the **node**
name: the `nixosConfigurations` key, the guest's hostname, and the OCI
repository its image is pushed to.

They differ because they have to. `@` is not legal in a `networking.hostName`
(nixpkgs types it as a DNS label, so a VM named `worker@1` fails its own option
type before anything is built), and in an OCI reference `@` introduces a digest.
`-` cannot replace `@` in the instance name either: template `worker-pool`
instance `1` and template `worker` instance `pool-1` would spell one string,
which is why systemd chose a separator that appears in neither half.

A template never has to know this. `index.lib.templates` derives the node name
and injects it as the `name` param, so the body is
`index.lib.mkVm({ name, ... })` and a template that names its VM anything else
is refused with the reason.

## Shape

- [`default.ix`](default.ix) — the named `web` VM, the `worker` template, and
  the two `instances` presets. The params are `port` and `shards`; `worker@1`
  takes both defaults and `worker@2` overrides both.
- [`worker.nix`](worker.nix) — the template's guest definition. Nothing in it
  names an instance: everything instance-specific arrives in the `worker`
  module argument, so one file serves every instance.
- [`web.nix`](web.nix) — an ordinary named VM, here to show that `ix apply`
  cannot tell a rendered instance from a declared VM.
- [`flake.nix`](flake.nix) — the seam:
  `index.lib.templates.renderConfig config` renders each instance through its
  template and merges the result with the named VMs. A config exporting neither
  `templates` nor `instances` comes back through it unchanged.

## Verify

```sh
# Each instance reports the identity and params it was rendered from:
ix shell worker-2 -- curl --silent http://127.0.0.1:8081/
# {"node":"worker-2","instance":"2","shards":4}

# The `shards` param really is nginx's worker count, not a label:
ix shell worker-2 -- nginx -T | grep worker_processes   # worker_processes 4;
```

The repo's own gate is the `vm-templates` group in `tests/default.nix`, which
forces every node this example renders to a system derivation without building
one, and additionally checks each guard in `lib/templates.nix` by making the
mistake it exists to catch. It lives there rather than in a `checks` output
here because that group is what CI already runs.

## Scale

Another worker is one line in the `instances` block:

```js
instances: {
  "worker@1": {},
  "worker@2": { port: 8081, shards: 4 },
  "worker@3": { port: 8082 },
},
```

`ix apply .#worker-3` creates it. Fixing a bug in `worker.nix` and applying
reaches all of them, and each keeps its own params — which is the property that
copy-pasted VM definitions cannot offer.
