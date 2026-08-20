# ix CLI

`ix` is the platform CLI for running your own VMs: you create them, attach to
them, and tear them down by name. This page is the verb
map and the mental model, not a flag reference. For flags on any verb, run
`ix <verb> --help`.

## Install

The official installer drops the `ix` binary on your `PATH`:

```sh
curl -fsSL https://ix.dev/install.sh | sh
```

Or install it with Nix from the
[`ix-cli`](https://github.com/indexable-inc/ix-cli) flake, which pins the same
`ix.dev` binary by content hash:

```sh
nix run github:indexable-inc/ix-cli -- ls        # run without installing
nix profile install github:indexable-inc/ix-cli  # install into your profile
```

As a flake input:

```nix
{
  inputs.ix.url = "github:indexable-inc/ix-cli";
  # then use ix.packages.${system}.default
}
```

Supported platforms: `aarch64-darwin` and `x86_64-linux`.

A binary installed by the script updates itself in place with `ix update`. A
Nix-managed binary lives in the read-only store and is refused: update the
`ix-cli` flake input or profile instead.

## Mental model

- **You own VMs.** A VM is a boot image wrapped in an ix boundary: networking,
  logs, shell, snapshots, lifecycle. Name them with
  `--name`, list with `ix ls`, address every later command by that name.
- **`ix apply` is the one create-and-converge verb.** Each positional target is
  classified by shape. A Nix installable (`.`, `.#web`, a `github:` ref, a
  `/nix/store` path) builds your repo's NixOS config and converges the VM in
  place, the same contract as `nixos-rebuild switch`: re-running reconverges,
  with no separate switch command. A snapshot UUID warm-restores that snapshot.
  Anything else (`ix/base:latest`, `ubuntu:24.04`) boots an OCI image as a
  long-running VM. Bare `ix apply` defaults to `.`: the config push is the
  common case.
- **`ix run` is the imperative one-off.** It boots a fresh VM, runs one command,
  streams its output, and leaves the VM up. Reach for `apply`
  for a config you own and re-converge; reach for `run` for a quick command.
- **Evaluation and build are remote.** `ix apply .#web` uploads your tree,
  evaluates it on a caller-owned evaluator VM (`ix-eval-<region>`), and the
  target VM builds the system from that same uploaded source: no nix on your
  machine, ever. To hand a VM a prebuilt answer instead, name the store path:
  `ix apply $(nix build --print-out-paths .#web)` exports your built closure
  and the guest builds nothing, which matters because nodes of one deployment
  usually share almost all of their closure. Apply never builds locally and
  never substitutes to make this true: either the path is already in your
  store or nothing changes.
- **There is no fleet concept.** A project defines one VM and `ix apply`
  converges it. The standalone multi-VM `ix-fleet` tool is deprecated
  (indexable-inc/ix#8306, see [fleet.md](fleet.md)). There is no `ix down`,
  `ix health`, `ix diff`, or `ix fleet`. Single-VM teardown is `ix rm` (or
  `ix stop` to keep it).

## Verbs

Hidden/debug verbs are omitted; nested
actions show as `verb <action>`.

| group | verb | what it does |
| --- | --- | --- |
| Provision | `apply [targets]` | One create-and-converge verb, classified by target shape: a Nix installable builds + converges VMs from your NixOS config like `nixos-rebuild switch` (bare `apply` defaults to `.`), a snapshot UUID warm-restores, any other ref boots that OCI image as a long-running VM. |
| | `run -- <cmd>` | Boot a fresh VM, run `<cmd>`, stream output, leave the VM up; exits with the command's code. |
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
| | `logs <vm>` | Read captured streams: `workload` (default), `kernel`, `diagnostic`, `platform`. |
| Images / source | `image <ls\|push\|rm>` | Manage registry images; bare push refs land under `registry.ix.dev/<you>/`. |
| | `source <ls\|rm>` | List or remove CAS-backed uploaded source trees. |
| Networking | `group <create\|rm\|ls\|add\|rm-member\|members>` | East-west groups: decide which VMs reach each other privately. |
| | `net up <group>` | Bring up the Linux overlay for a group (TUN device, `<name>.ix.internal` DNS). Needs sudo / `CAP_NET_ADMIN`. |
| Secrets | `secret <set\|check\|ls\|rm>` | Store write-only secrets; `set` reads the value from a prompt/stdin/file, never the command line. |
| Account | `login` | Sign in through the ix website; also switches profiles. |
| | `billing <status\|top-up\|usage>` | View balance, add funds, inspect usage. |
| | `update` | Replace this binary with the latest published release (script installs only; Nix installs update through the flake). |
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

- [fleet.md](fleet.md): the deprecated multi-VM `ix-fleet` tool (indexable-inc/ix#8306).
- [lifecycle.md](lifecycle.md): provision -> run -> stop -> snapshot -> rm.
- [networking.md](networking.md): groups, the overlay, and public reachability.
- [secrets.md](secrets.md): the write-only secret store and default attachment.
- [overview.md](overview.md): where `ix` sits in the platform.
