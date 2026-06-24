# ix CLI

`ix` is the platform CLI for running your own microVMs: you create them, attach to
them, and tear them down by name. This page is the verb
map and the mental model, not a flag reference. For flags on any verb, run
`ix <verb> --help`.

## Mental model

- **You own VMs.** A VM is a boot image wrapped in an ix boundary: networking,
  logs, shell, snapshots, lifecycle. Name them with
  `--name`, list with `ix ls`, address every later command by that name.
- **`ix up` is declarative.** It builds your repo's NixOS config and converges the
  VM in place, the same contract as `nixos-rebuild switch`: re-running
  reconverges, with no separate switch command. Default
  target is `.`.
- **`ix new` / `ix run` are imperative one-offs.** `new` boots an OCI image as a
  long-running VM; `run` boots a fresh VM, runs one command,
  streams its output, and leaves the VM up. Reach for `up`
  for a config you own and re-converge; reach for `new`/`run` for a quick image.
- **Fleets are a separate tool.** Multi-VM declarative fleets live in `ix-fleet`
  (`nix run .#ix-fleet -- --plan plan.json <verb>`), not in `ix`. See
  [fleet.md](fleet.md). There is no `ix down`, `ix health`, `ix diff`, or
  `ix fleet`. Single-VM teardown is `ix rm` (or `ix stop` to keep it).

## Verbs

Hidden/debug verbs are omitted; nested
actions show as `verb <action>`.

| group | verb | what it does |
| --- | --- | --- |
| Provision | `new [image]` | Boot an OCI image (default `ix/base:latest`) as a long-running VM, or warm-restore a snapshot UUID. |
| | `run -- <cmd>` | Boot a fresh VM, run `<cmd>`, stream output, leave the VM up; exits with the command's code. |
| | `up [targets]` | Declaratively build + converge VMs from your NixOS config, like `nixos-rebuild switch`. |
| | `init` | Write a minimal `flake.nix` + `ix.nix` in the current dir; existing files untouched. |
| Inventory | `ls` | List your VMs: name, state, region, address, usage. Read-only inventory. |
| | `start <vm>` | Resume stopped VMs; does not create or change the image. |
| | `stop <vm>` | Stop runtime but keep the VM startable; not deletion. |
| | `restart <vm>` | Power-cycle the VM, same identity. |
| | `rm <vm>` | Delete VMs, disks, runtime state. Alias `delete`. Stop instead if you may restart. |
| | `snapshot [vm] [create\|ls]` | List or create saved VM states as recovery points. |
| | `vm <describe\|set\|revert>` | Inspect placement, toggle internet ingress/egress, or revert by booting a new VM from a snapshot. |
| Access | `shell <vm>` | Interactive shell in the guest; create or `--attach` a session. |
| | `console <vm>` | Attach to the workload console for live stdin (REPL, installer). |
| | `serial <vm>` | Host-terminated serial console: the rescue line when `ix shell` / the agent is dead. |
| | `port-forward <vm> <l:r>` | Private dev tunnel from your laptop to a VM port; not public ingress. |
| | `logs <vm>` | Read captured streams: `workload` (default), `kernel`, `diagnostic`, `platform`. |
| Images / source | `image <ls\|push\|rm>` | Manage registry images; bare push refs land under `registry.ix.dev/<you>/`. |
| | `source <ls\|rm>` | List or remove CAS-backed uploaded source trees. |
| Networking | `group <create\|rm\|ls\|add\|rm-member\|members>` | East-west groups: decide which VMs reach each other privately. |
| | `net up <group>` | Bring up the Linux overlay for a group (TUN device, `<name>.ix.internal` DNS). Needs sudo / `CAP_NET_ADMIN`. |
| | `share <vm> <port>` | Publish a guest port on a public or email-gated (`--to`) share hostname. |
| Secrets | `secret <set\|check\|ls\|rm>` | Store write-only secrets; `set` reads the value from a prompt/stdin/file, never the command line. |
| Account | `login` | Sign in through the ix website; also switches profiles. |
| | `billing <status\|top-up\|usage>` | View balance, add funds, inspect usage. |
| Federated | `resources <ls\|get\|act\|attach>` | Drive remote federated TUI resources (agent terminals) over QUIC. |

Hidden verbs exist for debugging (`doctor`, `reload`, `sysrq`, `trace`, `config`,
`system`); they take `--admin`/`IX_ADMIN` or are otherwise internal and are not
part of the day-to-day surface.

## Flags

This page does not transcribe flags. Run `ix <verb> --help` for the authoritative,
current flag list on any verb. Four global flags apply everywhere:

| flag | env | effect |
| --- | --- | --- |
| `--profile` | `IX_PROFILE` | Select a config profile. |
| `--debug` | `IX_DEBUG` | Enable CLI debug tracing. |
| `--admin` | `IX_ADMIN` | Use admin privileges (bypasses ownership checks). |
| `--message-format` | - | Output format: `human` (default), `short`, `json`. |

## See also

- [fleet.md](fleet.md): multi-VM declarative fleets via the separate `ix-fleet`.
- [lifecycle.md](lifecycle.md): provision -> run -> stop -> snapshot -> rm.
- [networking.md](networking.md): groups, the overlay, and shares.
- [secrets.md](secrets.md): the write-only secret store and default attachment.
- [overview.md](overview.md): where `ix` sits in the platform.
