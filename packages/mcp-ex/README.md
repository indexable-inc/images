<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
    <img src="assets/hero.svg" width="720" alt="exec runs cells as supervised BEAM processes over one persistent workspace; a blocked cell stalls only itself and job outcomes come back as channel notifications">
  </picture>
</p>

# ix-mcp-ex

What if a wedged cell could not freeze your agent's whole kernel? ix-mcp-ex is
an MCP server whose REPL **is Elixir**: one tool, `exec`, runs code
against a shared, persistent workspace of bindings, and every cell executes as
its own supervised BEAM process. The namespace survives across calls, jobs that
outlive a short budget keep running in the background, and no cell -- however
stuck -- can delay any other, because the scheduler preempts instead of
cooperating.

## Quickstart

```
nix run github:indexable-inc/index#mcp-ex        # MCP over stdio
```

From a clone (`git clone https://github.com/indexable-inc/index`):

```
nix run .#mcp-ex
```

Point an MCP client at that command and call `exec`.

## The main tool

`exec(code, budget \\ 15, intent, workspace \\ "main")` evaluates `code` on a
persistent workspace and waits up to `budget` seconds. Finished in time: you get output
and the rendered result. Still running: it continues as a background job and
you get a handle. Job control needs no extra tools, because the registry is
in-language -- the `Jobs` module is aliased in every cell:

```elixir
Jobs.tail("ab12", 20)      # last lines of a run's output
Jobs.grep("ab12", ~r/err/) # filter it
Jobs.await("ab12")         # block this cell (only this cell) until done
Jobs.result("ab12")        # the value, as a term
Jobs.cancel("ab12")        # kill it, and every OS process it spawned
Jobs.history()             # recent runs, grouped by session/topic
```

Bindings, aliases, and modules you define persist: you are building a session,
not running one-shot snippets. `Api.api("tail")` and `Api.help(Jobs, :tail)`
are the discovery surface, generated live from module docs.

### Named workspaces: isolation for concurrent agents

One kernel is one MCP connection, and a Claude Code subagent runs on its
parent's connection. So without isolation the parent's cells and every
subagent's cells write to one binding map, and common names collide: a parent
holding `body` as an HTML string had it replaced by a subagent's `body`, a
list of a file's lines, and the parent's next `<>` raised `not a bitstring`
(#3967).

Nothing in the MCP protocol says who is calling (a `tools/call` carries only a
per-call `claudecode/toolUseId`), so the kernel cannot partition bindings by
agent automatically. The fix is explicit: `exec` takes `workspace: "name"`,
and every agent that might run concurrently with another passes its own name
on every call. A named workspace is its own supervised evaluator process with
its own binding map, env, provenance and checkpoint row, created on first use
(`IxMcp.Workspaces.Supervisor` + a Registry); `Workspace.list/0`,
`Workspace.new/1` and `Workspace.drop/1` manage them from cells. Only the
binding map and env are isolated -- modules, processes, ETS and the filesystem
stay BEAM-global.

Inside the still-shared `"main"` workspace, the kernel refuses to let a
takeover be silent. Every write
records its cell's job, intent, value shape and time, and a cell writing over
another cell's variable is reported to both sides:

```
-- warning: shared binding: `body` was bound just now by job 7c5c9b57
   (intent: "agent A: render the dashboard body") as a 35-byte binary; this
   cell rebinds it as an 18-element list.
```

and then, to the cell that goes on to use it, before it uses it:

```
-- warning: shared binding: `body` changed type under this workspace: job
   3fab397b (intent: "agent B: read a nix file into lines") rebound it just
   now from a 35-byte binary to an 18-element list, over the value job
   7c5c9b57 (intent: "agent A: render the dashboard body") bound just now.
```

A same-typed takeover is a `note` on the writing side only; nothing downstream
is about to raise over it. `Ix.bindings()` lists every name with the cell that
bound it, which is the fast answer to "whose value is this?". Modules get the
same treatment: they are global to the BEAM whichever cell defines them, so a
redefinition now names the cell that had it, next to the compiler's own
`redefining module Page`.

## Tools

The MCP surface is exactly one tool; everything else the Python server
exposed as tools is an in-language callable, pre-aliased in every cell like
`Jobs` (the server instructions delivered at MCP initialize teach the same
list):

| tool | what it does |
|---|---|
| `exec` | run a cell; budget-then-background |

| in-cell callable | what it does |
|---|---|
| `Read.file(path, first \\ nil, last \\ nil)` | a file, optionally a 1-based line range |
| `Image.read(path)` | a PNG/JPEG/GIF/WebP as a value; a cell result carrying images puts real MCP image blocks on the reply (base64 + mime), which the client renders as pictures. `Image.from_binary/2` wraps generated bytes |
| `Workspace.new("name")` | an isolated named REPL (also created implicitly by `exec workspace:`); `Workspace.list/0` shows live ones, `Workspace.drop/1` deletes |
| `Cmd.run(cmd, args, opts)` | `System.cmd/3` with EOF stdin and the launch dir as default `cd:`; returns `{out, status}` and reports a nonzero exit as a note on the reply. `Cmd.run!`/`Cmd.sh!` raise on nonzero instead |
| `Memory.remember(slug, desc, opts)` | durable memory facts in the weave store at `WEAVE_MEMORY_STORE`; `supersedes:`/`relates:` write typed edges for `Memory.graph/1`; recall rich rows via `Memory.recall/2` (regex), `Memory.semantic/2` (embedding similarity, a resident `weave recall --stdin`), or `Memory.query/1` (Datalog); `Memory.verify/2` records re-check receipts |
| `Memories.search(query, opts)` | repo-local memory via the `memories` CLI: a `%Memories.Results{}` of ranked `%Memories.Hit{}` structs plus the `roots` it resolved as `%Memories.Root{path, exists, memories}` rows, over every `.memories/` directory (this repo, its parents, `~/.memories`). Empty `hits` is an answer (there is a score floor); the row counts tell a real miss from a search that covered nothing. Ranked already, ties broken by slug, so there is no rank step; `dirs:` is a list that replaces the default roots, `Memories.roots/1` resolves them without a query, `Memories.remember/3` writes a memory (`by:`/`how:` required for the default `genre: :memory`, becoming its first `validated` receipt) and `Memories.validate/2` records a later re-check |
| `Ix.trace()` | stack dump of every job's processes, from outside |
| `Ix.restart()` | cancel jobs (sparing the calling cell), restart every workspace, restore bindings from each checkpoint |
| `PrWatch.start(pr, cwd, interval \\ 15, timeout \\ 3600)` | watch a PR via `gh`, notify on merge/close/timeout |
| `Issues.pickup(3880)` | claim an issue atomically before working it (also `"owner/repo#n"`); a lost claim names the session that won |
| `Requests.post("review PR #42", body)` | offer any unit of work to every agent on the host (#3883); `Requests.pickup(id)` claims it atomically, `Requests.done(id)` finishes it, `Requests.list()` shows the board, open first |
| `Sessions.list()` | the session directory: every kernel instance on this host, with heartbeat liveness |
| `Sessions.send(id_or_name, text)` | message another session (`Sessions.broadcast/1` for all); arrives as a `source="sessions"` channel event within seconds |
| `Tui.act(uri, send_keys, peer \\ nil)` | drive a federated TUI resource via `ix-resource-cli` |

Because cells are separate BEAM processes, `Ix.trace/0` and `Ix.restart/0`
work from a fresh cell even while other jobs run or wedge -- the recovery
path that used to justify out-of-band tools needs none.

Every `tools/call` lands in a local SQLite action log
(`~/.local/state/ix-mcp-ex/actions.db`): a `sessions` row per server
instance (created lazily on first use), a `topics` row per topic (a
timeline -- repeated names make new rows), and an `actions` row per call
referencing both.

## Shell pipelines as data: `IxMcp.Stdlib.Sh`

A Bash pipeline is the least observable thing an agent can run: `a | b | c`
reports one exit status out of three, discards each stage's stderr unless
someone remembered `2>&1`, and re-splits interpolated words on whitespace. The
result is that a *silent false negative* is the default failure mode -- `rg`
exiting 2 on a broken pattern is byte-identical to "no matches found".

`Sh` removes the shell from the loop. Each stage is spawned from its own argv
list, joined to its neighbours by a real OS FIFO, with its own stderr file:

```elixir
Sh.pipeline([~w(rg -n TODO .), ~w(cut -d: -f1), ~w(uniq -c)]) |> Sh.run()
```

Every stage's `rc`, `stderr` (capped, with the original byte count), and
`duration_ms` come back on the `Result`. `Sh.ok?/1` asks the question a shell
cannot -- did *every* stage exit 0 -- and `Sh.ok!/1` raises with the whole
stage table when one did not. `run/2` itself never raises for a command
failure: a missing binary, a nonzero exit and a timeout are all fields.

Intermediate output never enters the BEAM, so a pipeline streams: 12 MiB
crosses three stages with only the final byte count in memory (backpressure is
the kernel's pipe buffer, exactly as in a shell). Bodies go in on `stdin:`,
never in argv -- Linux caps a single argv string at `MAX_ARG_STRLEN` = 131072
bytes however large an `ARG_MAX` the host advertises, and `cmd/2` refuses an
oversized word up front rather than letting the spawn die with a bare
"Argument list too long".

Two macros encode checks that are easy to skip and expensive to have skipped:

```elixir
Sh.mutate "advance the bookmark" do
  Sh.cmd(~w(jj bookmark set main -r @)) |> Sh.run()
verify
  Sh.ok!(Sh.cmd(~w(jj log -r main --no-graph -T commit_id))) == expected
end
```

`verify` clauses run after the mutation and must re-read the world; a mutation's
own output can claim success while nothing moved. A failed clause raises with
the clause's source text and both sides' values.

```elixir
Sh.watch("gate verdict",
  pattern: ~r/^gate: (PASS|FAIL)/m,
  must_match: "gate: FAIL 3 checks",
  must_not_match: "an UNDECLARED refusing instrument REFUSES (rc=1)")
```

A watcher refuses to arm until it has been shown to match a positive control
*and* reject a negative one, and literal patterns are validated at compile
time. The negative control is the one people skip, and it is exactly what
catches a pattern built from failure vocabulary: `~r/REFUS/` happily matches
the *name of a passing test arm*. Anchor to the runner's own verdict line.

## Why we left Python

The predecessor ([`packages/mcp`](../mcp)) runs cells on one asyncio IPython
kernel. It works, and a lot of engineering keeps it working, all of it
compensating for the same four runtime facts:

1. **Cooperative scheduling.** One synchronous call -- a `subprocess.run`, a
   store flusher's blocking `put_blob` -- freezes the event loop, and with it
   every session's jobs and even the status probe you would use to diagnose
   it. The kernel's docs spend paragraphs teaching agents to wrap calls in
   `asyncio.to_thread` because one mistake stalls the fleet. On the BEAM,
   scheduling is preemptive and per-process: a cell that blocks forever costs
   one process, and the chaos test in this package proves the next job starts
   on time anyway.
2. **Silent task death.** An unobserved asyncio `Task` exception parks
   invisibly until something polls `.result()`. Here a crashed cell is a
   monitored process whose exit reason -- a crash report carrying the actual
   error and state -- is pushed to the client as a channel notification the
   moment it happens.
3. **Restart blast radius.** Restarting the shared Python kernel kills every
   session and fleet riding it, and tracing a wedged loop requires
   faulthandler signal hacks from outside. `Ix.trace()` here is
   `Process.info/2` on live processes; `Ix.restart()` is a supervisor
   restarting one process subtree while an ETS checkpoint hands the bindings
   straight back.
4. **No supervision.** Restart budgets, escalation, crash-with-state reports:
   hand-rolled or absent in the Python runtime. OTP gives all of it
   declaratively; this server's whole recovery story is its supervision tree.

The orchestration problems were Erlang-shaped all along. The rewrite does not
add machinery to get these guarantees; it deletes the machinery the old
runtime needed to approximate them.

### Where `nu()` went

Nowhere: it is deliberately gone, and nothing replaced it. The Python kernel
needed a blessed shell wrapper because Python subprocess ergonomics plus the
one-shared-event-loop hazard demanded a managed, non-blocking, cancellable
path for every external command. In a real language REPL on a preemptive
runtime that is redundant surface area: a cell that genuinely needs a
subprocess writes `System.cmd/3` (or opens a `Port`) itself, in-language, and
blocks nobody. The one behavior from that world that still matters survives
the cut: cancelling a job kills the OS process tree its cells spawned, so jobs
never leak orphans.

## Architecture

```
IxMcp.Supervisor (one_for_one)
├── IxMcp.ActionLog        SQLite action log: sessions / topics / actions
├── IxMcp.Session          this instance's session/topic ids + labels
├── IxMcp.Checkpoint       ETS keeper for workspace state (survives restarts)
├── IxMcp.Workspace        the shared binding + Macro.Env every cell sees
├── IxMcp.Jobs.Registry    id -> job process
├── IxMcp.Jobs.Supervisor  (DynamicSupervisor) one child per cell/job
│   └── IxMcp.Jobs.Job*    spawn_monitor's the evaluation, owns its output ETS
├── IxMcp.Jobs.History     ordered record of every run
├── IxMcp.MCP.Notifier     server-initiated notification fan-out
├── IxMcp.PrWatch.Supervisor (Task.Supervisor) one task per PR watch
├── IxMcp.IssueWatch       new-issue channel feed (stdio-gated, `gh search issues`)
├── IxMcp.SessionWatch     session heartbeat + message and request-feed delivery (stdio-gated)
├── IxMcp.Inbox.Watcher*   new-message feeds: Beeper chats, mail (stdio-gated, credential-gated)
└── IxMcp.MCP.Stdio        newline-delimited JSON-RPC on stdin/stdout
```

The evaluator copies the design of Livebook's `Livebook.Runtime.Evaluator`
(persistent binding + `Macro.Env` via `Code.eval_quoted_with_env/4` with
`:prune_binding`, per-cell output capture through a group-leader IO proxy)
rather than depending on Livebook, which is an application, not a library.
The group leader doubles as the job's process-tree tag: it is how tracing
finds a job's spawned processes and how cancellation finds the ports whose OS
process trees must die.

Cells are gated by the compiler: code that does not parse is rejected with the
compiler's diagnostic and never evaluated; warnings ride along in the result.
There are zero runtime dependencies -- OTP's own `JSON` module is the whole
wire format.

## Tests

```
mix test                          # local, includes the chaos suite
nix build .#mcp-ex.tests.elixir   # sandboxed gate: types, format, credo, tests
nix build .#mcp-ex.tests.smoke    # real stdio initialize/tools-list exchange
```

The chaos suite runs the motivating scenario live: a cell blocks forever,
other jobs keep running, `Ix.trace()` shows the wedged frame, restart
recovers the bindings, and no spawned OS process outlives its job.

## RLM primitives (Ctx / LM / EventLog)

Context that does not fit in a window lives here as a variable, and the model
navigates it programmatically instead of ingesting it -- the shape from
*Recursive Language Models* (arXiv:2512.24601), which the kernel's persistent
REPL was already most of the way to.

    log   = Ctx.file!("/var/log/huge.log")   #Ctx<9f2ab1c4 41.2 MB 812004 lines "...">
    parts = Ctx.chunks(log, 32)              # line-aligned handles, no bytes
    parts |> LM.map(fn _ -> "Any failure here? One line." end, concurrency: 8)

    LM.budget()                      # what it cost
    EventLog.events(kind: :lm_ask)   # what it did

Three rules, each load-bearing:

* A handle RENDERS AS METADATA. Bytes reach a window only through `Ctx.read/2`
  (hard-capped) or by being handed to a sub-model. Without this the scheme
  collapses back into ingesting the context.
* Calls are MEMOIZED as derivations, keyed by blake3 over (model, prompt,
  context ids). Handle ids are `ix_hash::Content`, the same address jj's
  `FileId` uses over the same bytes, so cached answers transfer across sources
  and machines, and re-analysing a log that grew pays only for the chunks that
  are new.
* The budget FAILS CLOSED: `{:error, :budget_exhausted}`, never a truncated
  context and never a silently dropped sub-call.

`IxMcp.RLM` carries the design note; `IxMcp.LM.Stub` runs the whole thing with
no network and no key.

## The grown stdlib

A cell can `defmodule` anything, and that module lives as long as the BEAM
does. What it cannot do is survive a restart, be found by the next session, or
be trusted by a third one -- so a useful recipe gets retyped from memory, and
retyping is where it breaks. `IxMcp.Stdlib` is the promotion path: a module
under `lib/ix_mcp/stdlib/` is compiled with the kernel, gated by everything
that gates the kernel, and aliased into every cell by existing.

    Stdlib.modules()    # the residents
    Stdlib.fitness()    # per-function calls and outcomes, from the event log
    Stdlib.provenance(IxMcp.Stdlib.Forge)

Four rungs: SCRATCH (a cell's own `defmodule`, free and gone on restart),
RESIDENT (landed in that directory through CI), FITNESS (every call recorded
through `Stdlib.observe/3` into `rlm_events`, so a resident nobody uses can be
retired), PROVENANCE (a `## Provenance` section in the `@moduledoc` naming the
incident that created it, gated two-sided by the suite).

The first resident is `IxMcp.Stdlib.Forge`, which lands a change on the forge's
protected `main` and waits for its verdict:

    Forge.land(%{message: msg, files: %{"index/..." => body}},
      author: [name: "...", email: "..."])
    #=> {:passed, %{run_id: ..., landed_commit: ..., change_id: ...}}

It exists because four lanes copied that recipe by hand on one day and every
copy grew a different fault. Its `@moduledoc` names all four.

Aliases are read when a workspace is created, so a resident that landed after
this kernel booted needs a restart; reloading residents when ix main moves is
follow-up.

## Credits

Written by Claude Code (an AI agent), operated by the ix team. Evaluator
design after [Livebook](https://github.com/livebook-dev/livebook)'s
`Livebook.Runtime.Evaluator`.
