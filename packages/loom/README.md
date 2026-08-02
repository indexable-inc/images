# loom

Fork-the-workstation as the spawn primitive: one control VM you work in,
and every subagent runs `claude -p` inside a snapshot-fork of that VM,
created, watched, stopped, and woken through the plain `ix` CLI.

loom is a dogfood project for ix VMs. It deliberately builds almost
nothing: the multi-agent semantics, the browser rendering, and the VM
lifecycle all come from prior art, and this package only supplies the
glue between them.

## Create the control VM

```console
ix secret set anthropic_api_key
ix secret set loom_ix_token
ix new github:indexable-inc/index#loom --name loom \
    --secret-file anthropic_api_key=anthropic_api_key \
    --secret-file loom_ix_token=loom_ix_token
ix shell loom
loom
```

The target is an ordinary template per the ix template contract: a plain
`nixosConfiguration` in the index flake, no ix imports. Inside, you work
normally (claude through `ix-mcp serve`). Spawning a subagent from an
exec cell:

```elixir
Loom.spawn("audit packages/foo for the flaky test and fix it")
```

does, in order:

1. `ix snapshot loom` - full-VM capture of the control VM; you keep
   working through the pause. The command blocks until the snapshot is
   replication-confirmed and prints the bare snapshot id on a pipe.
2. `ix new <snapshot-id> --name loom-a1 --no-shell` - warm restore into
   a fresh VM. The fork carries the workspace byte-exact as of the
   spawn: uncommitted changes, build caches, credentials on disk.
3. The fork demotes itself: a `loom-fork-guard` systemd unit baked into
   the template compares the recorded VM identity with the current one
   on every boot, and on mismatch kills the cloned kernel, dashboard,
   and user session, and scrubs `/run/secrets`. Control never has to
   repair a fork imperatively; forks are safe by construction.
4. `ix shell loom-a1 --noninteractive -- claude -p <brief>
   --output-format stream-json --verbose` - the child process runs in
   the fork, but the process handle and the output stream live on the
   control VM, riding the existing ix data plane. No new transport.
5. The child's final result is delivered to the caller as a message;
   the fork drops to `ix stop` (disk-only billing). Messaging an idle
   agent runs `ix start` and resumes the same claude session
   (`--resume <session-id>`) in the same workspace.

The dashboard (`packages/dashboard`) renders every child as a live pane
in the browser, plus the agent tree, through the stock local-producer
path - the driving processes are local to the control VM, so no bridge
exists.

## Using it as a human

The interface is an `iex` session inside the control VM; agent events
land in your shell process mailbox.

```console
ix shell loom
loom                     # starts the packaged interactive Elixir session
```

```elixir
{:ok, id} = Loom.spawn("In /root/work/myrepo: fix the flaky test in foo_test. Commit on branch agent/fix-foo and push.")
flush()                  # {:loom, id, {:spawned, "loom-..."}} ... {:final, text}
Loom.status(id)          # phase, session_id, result, log tail
Loom.send_text(id, "also update the changelog")   # wakes an idle agent
Loom.list()
Loom.delete(id)          # reaps the fork VM
```

Real workloads with a private repo: set the control VM up ONCE - clone
the repo (with `gh auth login` or a deploy key), install the
toolchain, warm the caches. Every fork inherits all of it byte-exact
at spawn time, credentials included, so the brief just names the path.
Children run with `--dangerously-skip-permissions` (LOOM_CLAUDE_ARGS):
the fork is a disposable VM - it IS the sandbox - and each agent
pushes its own branch, so nothing merges without you.

Troubleshooting, from the live runs that built this:

| symptom | meaning | fix |
| --- | --- | --- |
| spawn takes minutes | self-snapshot replicating dirty deltas (5s-4min observed) | wait; keep the control VM's disk churn low |
| `{:failed, {:provision, {:exit, 1, "...spawning VMM (timed out)..."}}}` | platform restore flake | `Loom.delete(id)`, `ix rm loom-<id> --force` if present, respawn |
| `{:failed, {:preflight, ...}}` | fork unreachable - almost always the same-node hairpin | set `LOOM_IX_PREFIX=--admin` and `LOOM_RESTORE_ARGS="--on <other-node>"` |
| wake takes ~2.5min | cold CAS chunks on the fork's node | normal; warm wakes are ~5s |
| `child_exit 1` + `No conversation` in the tail | session transcript lost (pre-sync-fix forks only) | unrecoverable session: delete and respawn |
| `child_exit 1` + auth noise in the tail | key file missing/stale in the fork | check `/var/lib/loom/anthropic_api_key` on the control VM, respawn |
| stray `loom-*` VMs after a crash | driver died mid-flight | `ix ls`, `ix rm loom-* --force`; stopped forks bill disk only |

## Prior art this stands on

- `packages/agent-harness-ex` - the async-subagents semantics (Fable 5
  system card sec 8.15.3) as an OTP library.
- `packages/mcp-ex` - the kernel surface (`exec` cells) and the house
  pattern for wrapping external binaries; `IxMcp.Agents` is the local-
  process version of exactly this spawn model.
- `packages/dashboard`, `packages/tui` - the browser rendering stack.
- The ix CLI - the entire VM lifecycle. loom holds no SDK dependency;
  every verb is a short-lived `ix` invocation, which is the point of
  the dogfood.

## What is deliberately not here

- No Datalog / fact store (the Weave part this is "like" but simpler
  than): runtime state is OTP process state plus an NDJSON event log.
- No Ray: its stable-cluster membership model fights the
  fork/stop/wake VM lifecycle, and `ix shell --noninteractive` already
  is "run a function on another machine".
- No cross-VM pane transport, no BEAM distribution, no east-west
  groups in v1. All candidates for later; none are needed to ship.

## Measured (2026-08-01, prod us-west-1, in-guest driver)

Full loop green (round 7): spawn -> fork -> claude final -> sync'd
stop -> wake -> `--resume` same-session -> final -> stop -> delete.

- Fork spawn (request -> booted fork, incl. self-snapshot with
  replication confirm + restore + preflight): 5s best, 241s worst
  across 5 rounds - dominated by the snapshot's dirty-delta
  replication, not the restore.
- Child claude turn inside the fork: 3-5s.
- Wake (`ix start` of a stopped fork): 5s warm-chunk, ~2.5min cold.
- Platform findings from the runs (each with a loom-side mitigation
  in-tree): same-node guest-to-guest data-plane dials hairpin-fail
  (forks pin cross-node via `:restore_args` until a guest-reachable
  connect endpoint exists); `/run/secrets` does not exist on a
  restored fork's cold boot (credentials persist to disk instead);
  `ix stop` is crash-consistent, not flush-clean - a file written ~2s
  before stop came back 0 bytes (loom syncs the guest before every
  idle stop); one restore died in `spawning VMM (timed out)`.

## Testing

Three rungs, cheapest first:

1. `mix test` - the full lifecycle against a recording fake `ix`
   binary (`test/support/fake-ix`). Asserts the exact verb sequences
   (spawn = snapshot -> new -> shell; finish -> stop; wake = start ->
   shell --resume) and the failure arms. Counts calls; a green run with
   zero recorded verbs fails.
2. Live lifecycle from a workstation: set `LOOM_PARENT_VM` to any VM in
   your account and `Loom.spawn/2` runs the real path end to end with
   real snapshots and a real `claude -p` (needs `ANTHROPIC_API_KEY`
   reachable inside the fork). This is where spawn latency is measured,
   not guessed.
3. Template e2e: `ix new github:indexable-inc/index/<branch>#loom`
   boots the control VM from the branch before merge (template refs
   take branch revs).
