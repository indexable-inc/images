# VM lifecycle

Use `ix up` for NixOS configurations you own. Use `ix new` or `ix run` for an
imperative OCI image.

| Goal | Command |
| --- | --- |
| Converge the default fleet | `ix up` |
| Converge one explicit NixOS target | `ix up .#web` |
| Boot an OCI image | `ix new <image>` |
| Boot an image and run a command | `ix run <image> -- <cmd>` |
| Stop or start an existing VM | `ix stop <vm>` / `ix start <vm>` |
| Reboot an existing VM | `ix restart <vm>` |
| Save current state | `ix snapshot create <vm>` |
| Restore into a new VM | `ix vm revert <snapshot>` |
| Delete a VM | `ix rm <vm>` |

## Declarative convergence

With no explicit targets, `ix up` reads `ix.fleets.default`, uploads the
repository, and converges every declared node. Each system is built and
activated on its own target VM. Re-running `ix up` applies configuration
changes in place and rechecks the fleet's dependencies and health gates.

There is no separate fleet executor, switch command, or builder VM. Remove a
fleet by deleting its nodes with `ix rm`; `ix stop` only changes power state.

See [fleet.md](fleet.md), [cli.md](cli.md), and [images.md](images.md).
