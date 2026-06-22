# ix fleet

A forkable ix environment (RFC 0007). One [`ix.nix`](ix.nix) - an ordinary
NixOS module - is the source of truth for the per-VM environment, the fleet
topology, and an opt-in shared SMB volume that gives the whole fleet one Claude
(and ix) login.

## Run

```sh
ix up
```

This example declares a multi-node `ix.dev.fleet`. Omit that block and the same
`ix.nix` is a **single VM named `dev`** that `index.lib.mkDev` builds and
creates through `nix run .#up`; `default.nix` shows the composition. The fleet
below is the scale-up.

## Shape

- [`ix.nix`](ix.nix) is the module a user edits after `ix init`. Top-level
  NixOS config (`environment.systemPackages`, `programs.git.enable`) is the
  environment; `ix.dev.fleet` is the topology; `ix.dev.shared` turns on the
  identity volume. Claude Code and Codex are installed by default.
- [`default.nix`](default.nix) hands the module to `index.lib.mkDev`, passing
  `src = ./.` (the flake source the template wires as `self`).

`mkDev` reads `ix.dev` and desugars this into a `mkFleet` plan:

- `agent-0`, `agent-1`, `builder` — workload nodes carrying the module's
  environment on top of the dev base module (which ships our wrapped
  `claude-code` and `codex` via `lib/dev/agents.nix`).
- `file-server` — a dedicated node running `smbd`, exporting the share `dev`
  from `/var/lib/ix-dev-share`. Keeping it separate decouples the canonical
  credentials' lifecycle from the workload VMs, so recreating an agent never
  blips the share.
- A private east-west group (`ix-dev-shared`) so the share is reachable as
  `//file-server/dev` by hostname and never published.

## Shared login

`agent-0` and `agent-1` bind `~/.claude` and `~/.n` onto the volume, so the
first `claude login` on either agent logs in the whole fleet; a new replica
costs no extra auth. `builder` is in `ix.dev.shared.excludeNodes`, so it gets
neither the mount nor the shared identity - the per-VM opt-out - but it still
has the agents.

Only `~/.claude` and `~/.n` are shared, never the whole `~/.config`. The image's
`/etc/claude-code/managed-settings.json` policy stays in the image; the share
carries only credential/state, so the two layers do not collide.

## Recursion

Every node has `/ix` (this source). On the shared agents it is the volume's `ix`
directory (writable, fleet-wide); on `builder` it is a local writable copy. A
node can edit `/ix` and bring up its own fleet from the same module. (Shipping
the `ix` CLI on `PATH` inside the guest is the cross-repo follow-up RFC 0007
notes; this example places the source.)

## Tradeoffs

- The share is **guest-writable** by default (`ix.dev.shared.guestOk`) so
  `ix up` works without secrets plumbing, the same tradeoff
  [`multi-client-file-sharing`](../multi-client-file-sharing) documents. It is
  only reachable on the private group, never public. A real shared-auth volume
  should set `guestOk = false`, add a Samba user, and pass `credentials=`
  through a systemd `LoadCredential`.
- Any node on the volume can read the fleet's shared credentials. That is
  inherent to "one login for all VMs"; it is bounded to a single user's own
  fleet. `ix.dev.shared.ix` is the sharper opt-in: it hands out the ability to
  create more VMs.
