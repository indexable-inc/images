# agent-harness-ex

What does the best-performing multi-agent configuration in the Fable 5
system card look like when you build it out of the thing it was already
describing, processes with mailboxes?

`agent-harness-ex` is that harness as a plain OTP library: the card's
"async subagents" design (long-lived agents, immediate spawns, Send
Message / Wait for Message, a lead with create/delete/status tools) mapped
one-to-one onto supervised BEAM processes. It implements every semantic in
the card except the model call itself, which the host injects through a
single behaviour, so the library never talks to any API.

## Why this harness

The system card (packages/system-cards/cards/anthropic/
claude-fable-5-mythos-5.md, sec 8.15.3) evaluates three multi-agent
harnesses; async subagents is the one where the lead keeps its own task
tools and spawns asynchronous, long-lived subagents. Its reported wins over
a single agent (sec 8.15.5 and index#3700): BrowseComp 93.3 vs 88.0, and on
ProgramBench +7.9pp with 3.2x less critical-path latency to reach a 60
percent pass rate. Long-lived context is also the token argument: waking an
existing subagent costs less than re-establishing a fresh one per subtask.

The card's operating parameters, which are this library's defaults:

| Knob | Card (sec 8.15.3) | Here |
| --- | --- | --- |
| Per-agent token limit | 1M, no compaction | `token_budget: 1_000_000`, per spawn or per harness |
| Concurrent subagents | 4 (ProgramBench) | `max_concurrent: 4` |
| Lifetime subagents | 20 (ProgramBench) | `max_total: 20` |
| Subagent model | unspecified | `:model`, required per spawn (or `:default_model`), opaque to the harness |

## Semantics

* `create_subagent(harness, instructions, opts)` returns immediately with
  the new agent's id; the runner starts working in the background
  (lead-only tool).
* A subagent receives only the lead-provided instructions, never the
  original task description. That is structural: instructions are the only
  task-shaped input `Runner.run/2` gets.
* `send_message(harness, from, to, text)` queues; the recipient sees it
  after its next tool result, when its runner drains `checkpoint/2`.
  Any agent can message any other agent, including the lead.
* `wait_for_message(harness, id, timeout)` blocks until a message arrives
  (it drains the whole mailbox).
* When a runner returns `{:ok, final}`, the final response is delivered to
  the lead as a `:final` message and the subagent goes `:idle`. Messaging
  an idle subagent wakes it: the text becomes its new instructions.
* `delete_subagent/2` terminates the agent (and its runner task) and frees
  a concurrency slot; `subagent_status/1,2` answers `:working`, `:idle`, or
  `:terminated`.
* `add_usage/3` charges an agent's token budget and returns
  `{:error, :budget_exhausted}` once it is spent; runners stop there.

## Supervision tree

One harness instance, named `name` by the host:

    AgentHarness (Supervisor, :one_for_all)
    |-- Registry            name.Registry        {:agent, id} -> pid
    |-- Task.Supervisor     name.TaskSupervisor  one task per working runner
    |-- DynamicSupervisor   name.AgentSupervisor one AgentHarness.Agent per agent
    |   |-- Agent "lead"    the embedding session's mailbox (created at init)
    |   |-- Agent "sub-1"   mailbox + status + budget + runner task
    |   `-- ...
    `-- AgentHarness.Coordinator  name.Coordinator  roster, caps, admission

`:one_for_all` because the coordinator's roster, the registry, and the
dynamic supervisor's children describe each other; restarting one alone
would leave the survivors pointing at processes that no longer exist.

The agent GenServer never blocks on the model: the runner executes in a
`Task.Supervisor` task (`async_nolink`), so delivery, status, and deletion
stay responsive mid-sample. Agents are `restart: :temporary`; a crashed
agent is a `:terminated` roster entry (its runner's crash, by contrast,
does not kill the agent: it becomes an `:error` message to the lead and the
agent idles, ready for new instructions).

## Message wire shape

Everything agents exchange is one struct, `AgentHarness.Message`:

    %AgentHarness.Message{
      from: "sub-2",            # sender agent id ("lead" for the lead)
      to: "lead",               # recipient agent id
      text: "found the bug in ...",
      kind: :message,           # | :final | :error
      sent_at_ms: 1789000000000 # System.system_time(:millisecond)
    }

`:final` and `:error` are how a runner's ending reaches the lead; both
arrive through the same mailbox as ordinary messages, so the lead needs no
separate completion channel.

## The Runner seam

    defmodule MyApp.ClaudeRunner do
      @behaviour AgentHarness.Runner

      @impl true
      def run(instructions, ctx) do
        # sample ctx.model, execute tool calls, and around each tool result:
        {:ok, %{messages: msgs}} = AgentHarness.checkpoint(ctx.harness, ctx.agent_id)
        # splice msgs into the transcript; on the Wait for Message tool:
        {:ok, msgs} = AgentHarness.wait_for_message(ctx.harness, ctx.agent_id)
        # charge usage after each turn:
        {:ok, _left} = AgentHarness.add_usage(ctx.harness, ctx.agent_id, turn_tokens)
        {:ok, "final response text"}
      end
    end

The contract (also on the behaviour's doc): checkpoint after every tool
result, wait on the Wait tool, charge usage and stop when exhausted, return
`{:ok, final}`. The harness owns queueing, waking, status, caps, and
budget arithmetic; the runner owns the transcript and the API.

## Composing with ix-mcp-ex

The consumer this was built for is packages/mcp-ex (the workstation MCP
kernel), which takes the library as a mix path dependency. The intended
wiring, deliberately not yet exposed as MCP tools:

* Supervision: `{AgentHarness, name: IxMcp.Harness, runner: IxMcp.ClaudeRunner}`
  slots into `IxMcp.Application`'s one_for_one list next to the Jobs
  machinery it mirrors (`IxMcp.Jobs.Registry` / `IxMcp.Jobs.Supervisor`).
* The Jobs nursery stays the tool executor. A runner turn that wants shell
  or Elixir work calls `IxMcp.Jobs.run/2` exactly like a cell does, so
  subagent tool calls get the same nursery ownership, output capture, and
  cancellation story as every other kernel job (no parallel machinery).
* The lead is the MCP session itself. The server drains
  `AgentHarness.checkpoint(harness, "lead")` after each `tools/call` and
  appends the messages to that tool result, which gives the outer Claude
  the card's exact delivery rule.
* Planned tool surface (follow-up work, not in this package):
  `create_subagent`, `delete_subagent`, `subagent_status` (lead-only), and
  `send_message`, `wait_for_message` (all agents), each a thin adapter over
  the functions above. The runner implementation with the real Claude
  client also lives on the mcp-ex side; this package stops at the
  behaviour.

## Not yet decided / punted

* Roster names are never reused: a terminated subagent keeps its id so
  status stays answerable, and a new spawn under the same name is
  `:name_taken`. Pick a fresh name instead.
* The budget is enforced cooperatively (runners stop on
  `:budget_exhausted`); the harness does not kill an over-budget task.
* No persistence: a harness restart (`:one_for_all`) loses mailboxes and
  the roster. Fine for a kernel session; revisit if harnesses outlive
  sessions.
