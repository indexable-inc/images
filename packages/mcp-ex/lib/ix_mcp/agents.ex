defmodule IxMcp.Agents do
  @moduledoc """
  The kernel's fan-out surface: spawn and steer real agent CLIs (claude,
  codex, kimi) as async, long-lived subagents of this session, per the
  Fable 5 system card's top-scoring multi-agent harness (sec 8.15.3,
  index#3700). The session is the lead; `AgentHarness` owns the process
  semantics; this module is the lead-side facade the workspace prelude
  exposes as `Agents`.

  The topology is a bounded tree whose every level is a star, enforced
  structurally rather than by prompt. An agent reaches its own children and its
  own parent; it has no handle to a sibling, a grandchild, or anyone else's
  child, so messaging cannot become a mesh no matter what a brief asks for.

    * a child gets no MCP servers and so no spawn surface at all, unless the
      spawn asks for `kernel: true`, which gives it one kernel of its own
      (`IxMcp.Agents.Backend` builds either argv). Its own OS process, its own
      workspace: the namespace boundary is the process boundary, which is the
      one boundary that holds when modules are global to a VM (#3967, #3902);
    * every child carries IX_AGENT_DEPTH and `spawn/2` refuses past
      `max_depth/0` (default 2: lead, child, grandchild), so a nesting tree is
      bounded by construction rather than by its children's good behaviour;
    * children keep `--disallowedTools Agent,Task` at every level: fan-out goes
      through this module, never the native subagent tools;
    * child to parent is `IxMcp.Parent.send/1` (the cross-session bus) and
      parent to child is `send/2` here. There is still no child-to-child call.

  Depth 1 remains the default and the measured shape: the card's async-subagents
  harness scored best with a lead and a flat row of children, so `kernel: true`
  is opt-in per spawn until a rollout shows nesting earning its tokens
  (index#3700, index#4486).

  The idiom is non-blocking (the card's async-subagents variant beat the
  blocking orchestrator on score, latency, and tokens): spawn, keep
  working, and react to the `agent_finished` notification; `await/2`
  exists for the moments the lead genuinely cannot proceed without the
  answer.
  """

  import Kernel, except: [send: 2, spawn: 1]

  alias IxMcp.Agents.Control
  alias IxMcp.Agents.Events
  alias IxMcp.Agents.Metrics

  @harness IxMcp.Agents.Harness

  # Counting the lead as 0, so 2 is lead -> child -> grandchild. Two rather than
  # one because a grandchild is where delegation starts paying (a child that
  # finds three independent subproblems can hand them out instead of
  # serialising them), and not more than two because nothing has measured
  # deeper: the card's evidence covers the flat shape, and every level
  # multiplies token spend by its fan-out. IX_AGENT_MAX_DEPTH overrides on the
  # lead when a task genuinely nests further.
  @default_max_depth 2

  @type id :: String.t()
  @type backend :: :claude | :codex | :kimi

  @doc "The harness instance name (one per kernel)."
  @spec harness() :: atom()
  def harness, do: @harness

  @doc """
  Spawn a subagent; returns immediately with its id.

  Options: `:backend` (:claude default), `:model` (backend default
  otherwise), `:name`, `:cwd`, `:allowed_tools` (claude/kimi:
  auto-approved tools; default none beyond the CLI's own),
  `:permission_mode` (default "acceptEdits"), `:token_budget`,
  `:idle_timeout_ms`, `:kernel` (claude/kimi: give the child a kernel of its
  own, so it can run cells and fan out in turn; default false).

  The brief is everything the child will know: objective, output format,
  tool guidance, and boundaries. It never sees the lead's own task (the
  card's async-subagents rule).
  """
  @spec spawn(String.t(), keyword()) :: {:ok, id()} | {:error, AgentHarness.create_error()}
  def spawn(brief, opts \\ []) when is_binary(brief) do
    ensure_depth_budget!()

    backend = Keyword.get(opts, :backend, :claude)
    model = Keyword.get(opts, :model, default_model(backend))

    opts =
      opts
      |> Keyword.put(:backend, backend)
      |> Keyword.put(:model, model)

    case AgentHarness.create_subagent(@harness, brief, opts) do
      {:ok, id} ->
        Events.register_spawn(id, %{backend: backend, model: model, brief: brief})
        {:ok, id}

      {:error, _reason} = error ->
        error
    end
  end

  @doc """
  Queue a message for a child. Delivery follows the card: after the
  child's next tool result (claude backends, injected over stdin), or by
  waking it if idle. Codex children have no mid-run channel, so messages
  queue until the child idles and then wake it through `exec resume`.
  """
  @spec send(id(), String.t()) :: :ok | {:error, :not_found}
  def send(id, text) when is_binary(text) do
    AgentHarness.send_message(@harness, AgentHarness.lead_id(), id, text)
  end

  @doc """
  This kernel's place in the tree: 0 for a lead, n for a child spawned by a
  kernel at depth n-1. Read from IX_AGENT_DEPTH, which `IxMcp.Agents.Backend`
  sets on every child it builds.
  """
  @spec depth() :: non_neg_integer()
  def depth, do: env_int("IX_AGENT_DEPTH", 0)

  @doc "The deepest an agent may run, counting the lead as 0."
  @spec max_depth() :: non_neg_integer()
  def max_depth, do: env_int("IX_AGENT_MAX_DEPTH", @default_max_depth)

  @doc "The depth a child spawned from here runs at."
  @spec child_depth() :: pos_integer()
  def child_depth, do: depth() + 1

  @doc """
  Interrupt a child mid-turn. Best effort by nature: this is the Agent SDK
  control request the CLI understands on a stream-json stdin, and a build that
  does not understand it answers with an error event rather than acting. It is
  still the only way to stop a turn without killing the child and losing its
  session, which is why the alternative is not offered here.

  Codex children have no stdin channel at all, so they answer
  `{:error, :no_stdin_channel}` rather than appearing to have been stopped.
  """
  @spec interrupt(id()) :: :ok | {:error, :not_running | :no_stdin_channel}
  def interrupt(id) do
    case Control.lookup(id) do
      {:ok, %{stdin: :stream, runner: runner}} ->
        Kernel.send(runner, {:interrupt, request_id()})
        :ok

      {:ok, %{stdin: :closed}} ->
        {:error, :no_stdin_channel}

      :error ->
        {:error, :not_running}
    end
  end

  @doc """
  What every live child costs right now: its CLI process, everything that
  process spawned, and the per-agent totals. See `IxMcp.Agents.Metrics` for what
  the numbers do and do not say.
  """
  @spec top() :: [Metrics.agent()]
  def top, do: Metrics.tree()

  @doc "Status of every subagent this session created."
  @spec status() :: %{id() => AgentHarness.status()}
  def status, do: AgentHarness.subagent_status(@harness)

  @spec status(id()) :: {:ok, AgentHarness.status()} | {:error, :not_found}
  def status(id), do: AgentHarness.subagent_status(@harness, id)

  @doc """
  Block until the child's final response or error. Prefer reacting to the
  `agent_finished` notification; the card's non-blocking harnesses beat
  the blocking one on latency and tokens.
  """
  @spec await(id(), timeout()) :: {:ok, String.t()} | {:error, term()}
  def await(id, timeout \\ :infinity), do: Events.await(id, timeout)

  @doc "Terminate a child and free its concurrency slot."
  @spec delete(id()) :: :ok | {:error, :not_found}
  def delete(id), do: AgentHarness.delete_subagent(@harness, id)

  @doc "Recent normalized events of one child, newest first."
  @spec events(id()) :: [map()]
  def events(id), do: Events.events(id)

  @doc "Finished children and their final texts/errors."
  @spec report() :: %{id() => {:ok, String.t()} | {:error, term()}}
  def report, do: Events.finals()

  @doc "The who-spawned-whom graph as board-ready nodes/edges."
  @spec graph() :: %{nodes: [map()], edges: [[String.t()]]}
  def graph, do: Events.graph()

  defp ensure_depth_budget! do
    {depth, max} = {depth(), max_depth()}

    if depth >= max do
      raise "IxMcp.Agents.spawn/2 refused: this kernel is at depth #{depth} of a tree " <>
              "capped at #{max} (IX_AGENT_DEPTH / IX_AGENT_MAX_DEPTH), and the cap is " <>
              "structural, not advisory (index#3700). Report upward with Parent.send/1, or " <>
              "raise IX_AGENT_MAX_DEPTH on the lead if the work genuinely nests deeper."
    end
  end

  defp env_int(name, default) do
    case System.get_env(name) do
      nil ->
        default

      value ->
        case Integer.parse(value) do
          {n, ""} when n >= 0 -> n
          _junk -> raise "#{name} is #{inspect(value)}, not a non-negative integer"
        end
    end
  end

  defp request_id, do: "interrupt-" <> Base.encode16(:crypto.strong_rand_bytes(4), case: :lower)

  defp default_model(:claude), do: "sonnet"
  defp default_model(:kimi), do: "kimi-k3"
  defp default_model(:codex), do: :default
end
