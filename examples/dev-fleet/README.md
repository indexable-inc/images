# Dev fleet

A forkable dev environment (RFC 0007). One `dev.nix` is the source of truth for
three things: the per-VM environment, the fleet topology, and an opt-in shared
SMB volume that gives the whole fleet one Claude (and ix) login.

## Run

```sh
ix up
```

## Shape

- [`dev.nix`](dev.nix) is the spec a user edits after `ix dev init`. `env` is
  the environment applied to every node; `fleet.nodes` is the topology;
  `shared` turns on the identity volume; `selfSource` materializes `/ix`.
- [`default.nix`](default.nix) feeds the spec to `index.lib.mkDev`, passing
  `src = ./.` (the flake source the template wires as `self`).

`mkDev` desugars this into a `mkFleet` plan:

- `agent-0`, `agent-1`, `builder` — workload nodes carrying `env` on top of
  `development-base` (which ships our wrapped `claude-code` and `codex`).
- `file-server` — a dedicated node running `smbd`, exporting the share `dev`
  from `/var/lib/ix-dev-share`. Keeping it separate decouples the canonical
  credentials' lifecycle from the workload VMs, so recreating an agent never
  blips the share.
- A private east-west group (`ix-dev-shared`) so the share is reachable as
  `//file-server/dev` by hostname and never published.

## Shared login

`agent-0` and `agent-1` bind `~/.claude` and `~/.n` onto the volume, so the
first `claude login` on either agent logs in the whole fleet; a new replica
costs no extra auth. `builder` is in `excludeNodes`, so it gets neither the
mount nor the shared identity — the per-VM opt-out.

Only `~/.claude` and `~/.n` are shared, never the whole `~/.config`. The
image's `/etc/claude-code/managed-settings.json` policy stays in the image; the
share carries only credential/state, so the two layers do not collide.

## Recursion

Every node has `/ix` (this source). On the shared nodes it is the share's `ix`
directory (writable, fleet-wide); on `builder` it is a local writable copy. A
node can edit `/ix` and bring up its own fleet from the same spec. (Shipping
the `ix` CLI on `PATH` inside the guest is the cross-repo follow-up RFC 0007
notes; this example places the source.)

## Tradeoffs

- The share is **guest-writable** by default so `ix up` works without secrets
  plumbing, the same tradeoff [`multi-client-file-sharing`](../multi-client-file-sharing)
  documents. It is only reachable on the private group, never public. A real
  shared-auth volume should set `shared.guestOk = false`, add a Samba user, and
  pass `credentials=` through a systemd `LoadCredential`.
- Any node on the volume can read the fleet's shared credentials. That is
  inherent to "one login for all VMs"; it is bounded to a single user's own
  fleet. `ixAuth` is the sharper opt-in: it hands out the ability to create
  more VMs.
