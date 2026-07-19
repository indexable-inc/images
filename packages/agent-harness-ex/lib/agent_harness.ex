defmodule AgentHarness do
  @moduledoc """
  The Fable 5 system card's async-subagents multi-agent harness as a BEAM
  library (card sec 8.15.3; index#3700). One `AgentHarness` supervisor is
  one harness instance: a lead (the embedding session's mailbox) plus
  long-lived subagent processes it spawns, messages, and deletes.

  See README.md for the design doc. The semantics in one breath: spawning
  returns immediately; a subagent sees only the instructions the lead wrote;
  messages queue and are drained after the recipient's next tool result
  (`checkpoint/2`) or on demand (`wait_for_message/3`); a runner's final
  response lands in the lead's mailbox and the subagent idles until a new
  message wakes it; `delete_subagent/2` frees a concurrency slot.

  The library never calls a model API. The host injects an
  `AgentHarness.Runner` implementation, and everything here is plain OTP:

      children = [
        {AgentHarness, name: MyApp.Harness, runner: MyApp.ClaudeRunner}
      ]

  Options for `start_link/1` / `child_spec/1`:

    * `:name` (required) - atom naming this instance's process family.
    * `:runner` (required) - the `AgentHarness.Runner` module.
    * `:max_concurrent` - live subagent cap, default 4 (the card's
      ProgramBench setting).
    * `:max_total` - lifetime spawn cap, default 20 (ditto).
    * `:token_budget` - per-agent token budget, default 1_000_000 (the
      card runs each agent with a 1M-token limit, no compaction).
    * `:default_model` - fallback for spawns that omit `:model`; opaque to
      the harness. There is no built-in default: the model is a per-spawn
      decision.

  `create_subagent/3` options: `:name`, `:model`, `:token_budget`,
  `:runner` (per-spawn overrides), plus anything the host's runner wants to
  find in `ctx.opts`.
  """

  alias AgentHarness.Agent
  alias AgentHarness.Coordinator
  alias AgentHarness.Message
  alias AgentHarness.Names

  @type harness :: atom()
  @type agent_id :: String.t()
  @type status :: :working | :idle | :terminated
  @type create_error :: :max_concurrent | :max_total | :name_taken | :missing_model

  @default_max_concurrent 4
  @default_max_total 20
  @default_token_budget 1_000_000

  @spec child_spec(keyword()) :: Supervisor.child_spec()
  def child_spec(opts) do
    %{
      id: {__MODULE__, Keyword.fetch!(opts, :name)},
      start: {__MODULE__, :start_link, [opts]},
      type: :supervisor
    }
  end

  @spec start_link(keyword()) :: Supervisor.on_start()
  def start_link(opts) do
    name = Keyword.fetch!(opts, :name)

    config = %{
      runner: Keyword.fetch!(opts, :runner),
      max_concurrent: Keyword.get(opts, :max_concurrent, @default_max_concurrent),
      max_total: Keyword.get(opts, :max_total, @default_max_total),
      token_budget: Keyword.get(opts, :token_budget, @default_token_budget),
      default_model: Keyword.get(opts, :default_model)
    }

    children = [
      {Registry, keys: :unique, name: Names.registry(name)},
      {Task.Supervisor, name: Names.task_supervisor(name)},
      {DynamicSupervisor, name: Names.agent_supervisor(name), strategy: :one_for_one},
      {Coordinator, {name, config}}
    ]

    # one_for_all: the coordinator's roster mirrors the agents under the
    # DynamicSupervisor, and the Registry maps between them. Restarting any
    # one of the three alone would leave the survivors describing processes
    # that no longer exist, so a crash resets the whole instance.
    Supervisor.start_link(children, strategy: :one_for_all, name: name)
  end

  @doc "The lead's well-known agent id."
  @spec lead_id() :: agent_id()
  def lead_id, do: Names.lead_id()

  @doc """
  Spawn an async, long-lived subagent (lead-only tool). Returns immediately
  with the new agent's id; the runner starts working in the background.
  """
  @spec create_subagent(harness(), String.t(), keyword()) ::
          {:ok, agent_id()} | {:error, create_error()}
  def create_subagent(harness, instructions, opts \\ []) when is_binary(instructions) do
    GenServer.call(Names.coordinator(harness), {:create, instructions, opts})
  end

  @doc "Terminate a subagent and free its concurrency slot (lead-only tool)."
  @spec delete_subagent(harness(), agent_id()) :: :ok | {:error, :not_found}
  def delete_subagent(harness, id) do
    GenServer.call(Names.coordinator(harness), {:delete, id})
  end

  @doc "Status of every subagent ever created on this harness (lead-only tool)."
  @spec subagent_status(harness()) :: %{agent_id() => status()}
  def subagent_status(harness) do
    GenServer.call(Names.coordinator(harness), :status_all)
  end

  @spec subagent_status(harness(), agent_id()) :: {:ok, status()} | {:error, :not_found}
  def subagent_status(harness, id) do
    GenServer.call(Names.coordinator(harness), {:status, id})
  end

  @doc """
  Queue a message for `to` (any agent, including `lead_id/0`). It is
  delivered at the recipient's next `checkpoint/2` (i.e. after its next tool
  result), or immediately if the recipient is blocked in
  `wait_for_message/3`. Messaging an idle subagent wakes it: the text
  becomes its new instructions. The `from` id is host-trusted: nothing
  verifies the sender until the MCP surface adds enforcement.
  """
  @spec send_message(harness(), agent_id(), agent_id(), String.t()) ::
          :ok | {:error, :not_found}
  def send_message(harness, from, to, text) when is_binary(text) do
    call_agent(harness, to, fn pid ->
      Agent.deliver(pid, Message.new(from, to, text, :message))
    end)
  end

  @doc "Block until a message arrives for `id` (drains the whole mailbox)."
  @spec wait_for_message(harness(), agent_id(), timeout()) ::
          {:ok, [Message.t()]} | :timeout | {:error, :not_found}
  def wait_for_message(harness, id, timeout \\ :infinity) do
    call_agent(harness, id, fn pid -> Agent.wait_for_message(pid, timeout) end)
  end

  @doc """
  Drain `id`'s queued messages and read its remaining token budget. Runners
  call this after every tool result; hosts call it for the lead after every
  tool call of the outer session.
  """
  @spec checkpoint(harness(), agent_id()) ::
          {:ok, %{messages: [Message.t()], tokens_remaining: non_neg_integer()}}
          | {:error, :not_found}
  def checkpoint(harness, id) do
    call_agent(harness, id, fn pid -> {:ok, Agent.checkpoint(pid)} end)
  end

  @doc "Record token consumption against `id`'s budget."
  @spec add_usage(harness(), agent_id(), non_neg_integer()) ::
          {:ok, non_neg_integer()} | {:error, :budget_exhausted | :not_found}
  def add_usage(harness, id, tokens) do
    call_agent(harness, id, fn pid -> Agent.add_usage(pid, tokens) end)
  end

  defp call_agent(harness, id, fun) do
    case Registry.lookup(Names.registry(harness), {:agent, id}) do
      [{pid, _value}] -> fun.(pid)
      [] -> {:error, :not_found}
    end
  catch
    # The agent died between lookup and call, or mid-call (e.g. a concurrent
    # delete_subagent shutting it down); same answer as a missed lookup.
    :exit, {:noproc, _call} -> {:error, :not_found}
    :exit, {:shutdown, _call} -> {:error, :not_found}
    :exit, {{:shutdown, _reason}, _call} -> {:error, :not_found}
  end
end
