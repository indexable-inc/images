# VM lifecycle: which command?

This page is a decision guide for the VM lifecycle: creating, running, converging
config, power state, snapshots, and teardown. The single split that explains most
of it: swapping a VM's **image** means destroy-and-recreate, while changing
**config** on a VM that already exists is an in-place switch. Single VMs use the
`ix` CLI; many VMs at once use `ix-fleet` (see [fleet.md](fleet.md)). For full
flags, run the command with `--help`.

## I want to... -> command

| I want to... | Single VM (`ix`) | Fleet (`ix-fleet`) |
| --- | --- | --- |
| Create a VM from an OCI image | `ix apply <image>` (e.g. `ix/base:latest`) | `ix-fleet --plan p.json bootstrap` (from each node's `bootstrapImage`) |
| Boot a VM, run one command, keep it up | `ix run <image> -- <cmd>` | - |
| Bring up / converge from my NixOS config | `ix apply [target]` (default `.`) | `ix-fleet --plan p.json up` |
| Change config on a running VM (no recreate) | re-run `ix apply` | `ix-fleet --plan p.json switch` |
| Swap the VM's image | `ix apply <image>` again (recreates) | `ix-fleet --plan p.json replace` (or `up`) |
| Stop a VM but keep it | `ix stop <vm>` (`--force` hard-stops) | - |
| Start a stopped VM | `ix start <vm>` | - |
| Reboot a VM, same identity | `ix restart <vm>` (`--force` hard-stops first) | - |
| Save current state | `ix snapshot create <vm>` | `switch` snapshots each node first |
| Restore from a snapshot | `ix apply <snapshot-uuid>` or `ix vm revert <snapshot>` (boots a NEW VM) | - |
| Tear down | `ix rm <vm>` (alias `ix delete`) | `ix-fleet --plan p.json down` |
| See what would change | - | `ix-fleet --plan p.json diff` / `plan` |
| Check health | `ix doctor <vm>` | `ix-fleet --plan p.json health` |

## One verb, classified by target shape

`ix apply` is the create-and-converge verb; what it does follows from the shape
of each positional target:

- **A Nix installable** (`.`, `.#web`, a `github:` ref, a `/nix/store` path) is
  the **declarative** path: it builds the target NixOS system from your repo,
  creates the VM from `--base` if it does not exist yet, then activates that
  system in place. Re-running converges the VM to the current config: the same
  contract as `nixos-rebuild switch`, with no separate switch command for an
  existing VM. Bare `ix apply` defaults to `.`; several targets in one run (for
  example `.#web .#worker`) need `--build-vm`.
- **A snapshot UUID** warm-restores that snapshot into a new VM.
- **Anything else** (`ix/base:latest`, `ubuntu:24.04`) is the **imperative**
  path: you hand ix an OCI image and it boots a long-running VM around it. The
  image command becomes PID 1; ix wraps it with networking, logs, shell, and
  snapshots.

`ix run` stays the imperative one-off: it boots a fresh VM, runs your command
with output streamed back, exits with the command's exit code, and leaves the VM
running.

## Recreate vs switch: the key distinction

- **Swapping the image is destroy-then-create.** A VM's identity is bound to its
  image at creation, so changing the image means deleting the VM and creating a
  new one. Single VM: run `ix apply <image>` again. Fleet: `replace` always does
  delete-then-create, and `up` recreates when the image changed
  (`packages/ix-fleet/src/ix_fleet/__init__.py:426-431,820-841`).
- **Changing config on a running VM is an in-place switch.** No recreate, no new
  identity. Single VM: re-run `ix apply` (it switches in place). Fleet:
  `switch`, which snapshots each node first before activating the new closure
  (`__init__.py:956-975`; see [fleet.md](fleet.md)).

## Gotcha: there is no `ix down`

`ix` has no `down` verb. To tear down a single VM use `ix rm <vm>` (alias
`ix delete`), which destroys the VM identity, disks, and runtime state. To tear
down a fleet use `ix-fleet --plan p.json down`,
which removes nodes in reverse plan order
(`packages/ix-fleet/src/ix_fleet/__init__.py:1025-1027`). Note that `ix stop` is
power state only, not deletion: a stopped VM still exists and can be started again.

Restore is also not in-place: `ix vm revert <snapshot>` boots a **new** VM from
the snapshot and leaves the original untouched.

## See also

- [cli.md](cli.md): the full `ix` command surface.
- [fleet.md](fleet.md): managing many VMs with `ix-fleet`.
- [images.md](images.md): OCI images and the registry that `ix apply` boots from.
- [overview.md](overview.md): how the pieces fit together.
