defmodule IxMcp.Agents do
  @moduledoc """
  The kernel's fan-out surface: spawn and steer real agent CLIs (claude,
  codex, kimi) as async, long-lived subagents of this session, per the
  Fable 5 system card's top-scoring multi-agent harness (sec 8.15.3,
  index#3700). The session is the lead; `AgentHarness` owns the process
  semantics; this module is the lead-side facade the workspace prelude
  exposes as `Agents`.

  The topology is a depth-1 star, enforced structurally rather than by
  prompt:

    * children get no MCP servers and no built-in Agent/Task tools
      (`IxMcp.Agents.Backend` builds the lockdown argv), so no spawn
      surface exists below the lead;
    * every child runs with IX_AGENT_CHILD=1 and `spawn/2` raises under
      it, so even a future kernel-bearing child cannot recurse;
    * the surface has no child-to-child call: `send/2` is always
      lead-to-child, and a child's only outbound channel is its final
      response.

  The idiom is non-blocking (the card's async-subagents variant beat the
  blocking orchestrator on score, latency, and tokens): spawn, keep
  working, and react to the `agent_finished` notification; `await/2`
  exists for the moments the lead genuinely cannot proceed without the
  answer.

  That notification is durable, which is what makes the non-blocking idiom
  safe to rely on: a final is written to the outbox before anything tries to
  deliver it, so a child that finishes while no transport is attached is
  replayed on reconnect rather than lost with the CLI process that produced
  it. See `IxMcp.Agents.Events` and `IxMcp.MCP.Notifier`.
  """

  import Kernel, except: [send: 2, spawn: 1]

  alias IxMcp.ActionLog
  alias IxMcp.Agents.Events
  alias IxMcp.Session

  @harness IxMcp.Agents.Harness

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
  `:idle_timeout_ms`.

  The brief is everything the child will know: objective, output format,
  tool guidance, and boundaries. It never sees the lead's own task (the
  card's async-subagents rule).
  """
  @spec spawn(String.t(), keyword()) :: {:ok, id()} | {:error, AgentHarness.create_error()}
  def spawn(brief, opts \\ []) when is_binary(brief) do
    if System.get_env("IX_AGENT_CHILD") do
      raise "IxMcp.Agents.spawn/2 is lead-only: this kernel belongs to a spawned child " <>
              "(IX_AGENT_CHILD is set) and the topology is depth-1 by design (index#3700)"
    end

    backend = Keyword.get(opts, :backend, :claude)
    model = Keyword.get(opts, :model, default_model(backend))

    opts =
      opts
      |> Keyword.put(:backend, backend)
      |> Keyword.put(:model, model)

    case AgentHarness.create_subagent(@harness, brief, opts) do
      {:ok, id} ->
        Events.register_spawn(id, %{
          backend: backend,
          model: model,
          brief: brief,
          child_session: register_child_session(id, opts)
        })

        {:ok, id}

      {:error, _reason} = error ->
        error
    end
  end

  # The child's row in the host session directory, so a process outside
  # this BEAM (claude-html) can see the lead's subagents at all
  # (ENG-12004). Registration is strictly best-effort: the ledger note in
  # `Events` holds for the mirror too -- losing it must never cost the
  # child, so a dead or degraded ActionLog yields nil and the spawn
  # proceeds unlisted.
  @spec register_child_session(id(), keyword()) :: integer() | nil
  defp register_child_session(id, opts) do
    parent = Session.ids().session_id

    row =
      ActionLog.create_session(Keyword.get(opts, :name) || id, ActionLog, parent: parent)

    # 0 is the disabled log's reply, not a row.
    if row > 0, do: row
  catch
    :exit, _ -> nil
  end

  @doc """
  Queue a message for a child. Delivery follows the card: after the
  child's next tool result (claude backends, injected over stdin), or by
  waking it if idle. Codex children have no mid-run channel, so messages
  queue until the child idles and then wake it through `exec resume`.

  Steering invalidates the child's stored final: the answer to a message is
  the turn that message causes, so an `await/2` straight afterwards blocks
  for that turn rather than handing back the one before it. Until the new
  final lands the child has none, so `report/0` and `graph/0` show it working
  instead of reporting a result that predates the message.
  """
  @spec send(id(), String.t()) :: :ok | {:error, :not_found}
  def send(id, text) when is_binary(text) do
    # Existence first, so a mistyped id cannot drop a real child's final;
    # then the clear, strictly before the harness can deliver, so it can
    # never land on the very turn it is meant to wait for. A child deleted
    # inside that window loses its stored final and an await blocks to its
    # timeout -- the safe direction, the alternative being a confident stale
    # answer returned in 0.0s.
    with {:ok, _status} <- AgentHarness.subagent_status(@harness, id),
         :ok <- Events.expect_turn(id) do
      AgentHarness.send_message(@harness, AgentHarness.lead_id(), id, text)
    end
  end

  @doc "Status of every subagent this session created."
  @spec status() :: %{id() => AgentHarness.status()}
  def status, do: AgentHarness.subagent_status(@harness)

  @spec status(id()) :: {:ok, AgentHarness.status()} | {:error, :not_found}
  def status(id), do: AgentHarness.subagent_status(@harness, id)

  @doc """
  Block until the child's final response or error. Prefer reacting to the
  `agent_finished` notification; the card's non-blocking harnesses beat
  the blocking one on latency and tokens.

  A final this returns is not announced again: the reply carries it, so the
  durable row is acked here the way the exec reply path acks a job it waited
  out (#3934). A timeout acks nothing -- the child has not settled and its
  announcement is still owed.
  """
  @spec await(id(), timeout()) :: {:ok, String.t()} | {:error, term()}
  def await(id, timeout \\ :infinity) do
    case Events.await(id, timeout) do
      # Nothing was delivered, so nothing may be suppressed: the child has not
      # settled and its announcement is still owed. A real error final carries
      # the backend's text, never this bare atom, which only the await timeout
      # produces.
      {:error, :timeout} = timed_out ->
        timed_out

      final ->
        # This reply carries the final, so announcing it would say the same
        # thing twice (#3934). Strictly after the reply is in hand, exactly as
        # the exec reply path does it: a death before this line leaves the row
        # unacked and the announcement still fires.
        ack_final(id)
        final
    end
  end

  defp ack_final(id) do
    ActionLog.ack_outbox_ref(:agents, id)
  catch
    :exit, _reason -> 0
  end

  @doc "Terminate a child and free its concurrency slot."
  @spec delete(id()) :: :ok | {:error, :not_found}
  def delete(id), do: AgentHarness.delete_subagent(@harness, id)

  @doc """
  Recent normalized events of one child, NEWEST FIRST: index 0 is the latest
  line the child produced. Capped at the most recent few hundred, oldest
  dropped first, so this is a tail rather than a transcript.
  """
  @spec events(id()) :: [map()]
  def events(id), do: Events.events(id)

  @doc "Finished children and their final texts/errors."
  @spec report() :: %{id() => {:ok, String.t()} | {:error, term()}}
  def report, do: Events.finals()

  @doc "The who-spawned-whom graph as board-ready nodes/edges."
  @spec graph() :: %{nodes: [map()], edges: [[String.t()]]}
  def graph, do: Events.graph()

  defp default_model(:claude), do: "sonnet"
  defp default_model(:kimi), do: "kimi-k3"
  defp default_model(:codex), do: :default
end
