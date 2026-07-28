defmodule AgentHarness.Coordinator do
  @moduledoc """
  The roster and the caps: admission control for `create_subagent`, slot
  accounting for `delete_subagent`, and the status ladder for
  `subagent_status`.

  The roster remembers every subagent ever created, including dead ones, so
  status queries can answer `:terminated` instead of "not found" after a
  delete or a crash (a Registry lookup alone forgets the dead). Concurrency
  counts only live entries; the lifetime total only ever grows, which is
  what makes `max_total` a real cap rather than a high-water mark.

  The lead agent is created here at init so subagent final responses always
  have a mailbox to land in. It lives outside the roster: the caps and the
  status tool are about subagents.
  """

  use GenServer

  alias AgentHarness.Agent
  alias AgentHarness.Names

  @type config :: %{
          runner: module(),
          max_concurrent: pos_integer(),
          max_total: pos_integer(),
          token_budget: pos_integer(),
          default_model: term()
        }

  @spec start_link({atom(), config()}) :: GenServer.on_start()
  def start_link({harness, config}) do
    GenServer.start_link(__MODULE__, {harness, config}, name: Names.coordinator(harness))
  end

  @impl true
  def init({harness, config}) do
    state = %{
      harness: harness,
      config: config,
      total_created: 0,
      roster: %{},
      monitors: %{}
    }

    {:ok, _lead} = start_agent(state, Names.lead_id(), :lead, nil, config.default_model, [])
    {:ok, state}
  end

  @impl true
  def handle_call({:create, instructions, opts}, _from, state) do
    case admit(state, opts) do
      {:ok, id, model} -> do_create(state, id, model, instructions, opts)
      {:error, reason} -> {:reply, {:error, reason}, state}
    end
  end

  def handle_call({:delete, id}, _from, state) do
    case Map.get(state.roster, id) do
      nil ->
        {:reply, {:error, :not_found}, state}

      %{alive: false} ->
        # Idempotent: the slot is already free.
        {:reply, :ok, state}

      %{pid: pid} ->
        :ok = DynamicSupervisor.terminate_child(Names.agent_supervisor(state.harness), pid)
        {:reply, :ok, %{state | roster: mark_terminated(state.roster, id)}}
    end
  end

  def handle_call({:status, id}, _from, state) do
    case Map.get(state.roster, id) do
      nil -> {:reply, {:error, :not_found}, state}
      entry -> {:reply, {:ok, entry_status(entry)}, state}
    end
  end

  def handle_call(:status_all, _from, state) do
    statuses = Map.new(state.roster, fn {id, entry} -> {id, entry_status(entry)} end)
    {:reply, statuses, state}
  end

  @impl true
  def handle_info({:DOWN, ref, :process, _pid, _reason}, state) do
    case Map.pop(state.monitors, ref) do
      {nil, _monitors} ->
        {:noreply, state}

      {id, monitors} ->
        {:noreply, %{state | monitors: monitors, roster: mark_terminated(state.roster, id)}}
    end
  end

  # -- admission --

  defp admit(state, opts) do
    id = Keyword.get_lazy(opts, :name, fn -> default_id(state) end)
    model = Keyword.get(opts, :model, state.config.default_model)

    cond do
      state.total_created >= state.config.max_total -> {:error, :max_total}
      live_count(state.roster) >= state.config.max_concurrent -> {:error, :max_concurrent}
      Map.has_key?(state.roster, id) -> {:error, :name_taken}
      is_nil(model) -> {:error, :missing_model}
      true -> {:ok, id, model}
    end
  end

  defp do_create(state, id, model, instructions, opts) do
    case start_agent(state, id, :subagent, instructions, model, opts) do
      {:ok, pid} ->
        ref = Process.monitor(pid)

        next = %{
          state
          | total_created: state.total_created + 1,
            roster: Map.put(state.roster, id, %{pid: pid, alive: true}),
            monitors: Map.put(state.monitors, ref, id)
        }

        {:reply, {:ok, id}, next}

      {:error, {:already_started, _pid}} ->
        {:reply, {:error, :name_taken}, state}
    end
  end

  defp start_agent(state, id, role, instructions, model, opts) do
    args = %{
      harness: state.harness,
      id: id,
      role: role,
      instructions: instructions,
      runner: Keyword.get(opts, :runner, state.config.runner),
      model: model,
      token_budget: Keyword.get(opts, :token_budget, state.config.token_budget),
      opts: opts
    }

    DynamicSupervisor.start_child(Names.agent_supervisor(state.harness), {Agent, args})
  end

  # Default ids share the roster namespace with user-chosen names, so a
  # host that once named an agent "sub-2" must not lose its next anonymous
  # spawn to :name_taken; skip taken numbers instead of failing admission.
  defp default_id(state), do: default_id(state.roster, state.total_created + 1)

  defp default_id(roster, n) do
    id = "sub-#{n}"

    if Map.has_key?(roster, id) do
      default_id(roster, n + 1)
    else
      id
    end
  end

  # -- roster --

  defp live_count(roster) do
    Enum.count(roster, fn {_id, entry} -> entry.alive end)
  end

  defp mark_terminated(roster, id) do
    Map.update!(roster, id, &%{&1 | alive: false})
  end

  defp entry_status(%{alive: false}), do: :terminated

  defp entry_status(%{pid: pid}) do
    Agent.status(pid)
  catch
    # The agent died between the roster read and the call (its :DOWN is
    # still in flight); that is just :terminated observed early.
    :exit, _reason -> :terminated
  end
end
