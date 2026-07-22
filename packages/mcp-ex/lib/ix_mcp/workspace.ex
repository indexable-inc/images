defmodule IxMcp.Workspace do
  @moduledoc """
  Owns the shared evaluation context: the binding and `Macro.Env` every cell
  sees. A cell evaluates in its own process against a snapshot and merges its
  resulting context back on success -- last writer wins per variable, exactly
  the race concurrent Python cells already had on the shared namespace, minus
  the ability to freeze each other.

  Every merge is checkpointed into `IxMcp.Checkpoint` (an ETS table owned by a
  different process), so killing or restarting this server -- the analog of a
  kernel restart -- loses nothing: `init/1` restores the last checkpoint.
  """

  use GenServer

  # `Kernel` would shadow Elixir's; cells reach trace/restart as `Ix`.
  @prelude "alias IxMcp.Jobs; alias IxMcp.Api; alias IxMcp.Fleet; " <>
             "alias IxMcp.Read; alias IxMcp.Edit; alias IxMcp.PrWatch; alias IxMcp.Tui; " <>
             "alias IxMcp.TuiLocal; alias IxMcp.Gmail; alias IxMcp.Imsg; alias IxMcp.Contacts; " <>
             "alias IxMcp.Kernel, as: Ix; alias IxMcp.Agents; alias IxMcp.Memory; " <>
             "alias IxMcp.Ask; alias IxMcp.Cmd; alias IxMcp.Issues; alias IxMcp.Sessions; " <>
             "alias IxMcp.Serve; " <>
             "alias IxMcp.Requests"

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @doc "The current {binding, env} snapshot a cell should evaluate against."
  @spec snapshot() :: {Code.binding(), Macro.Env.t()}
  def snapshot do
    GenServer.call(__MODULE__, :snapshot)
  end

  @doc "Merge a finished cell's resulting context back into the shared state."
  @spec merge(Code.binding(), Macro.Env.t()) :: :ok
  def merge(binding, env) do
    GenServer.call(__MODULE__, {:merge, binding, env})
  end

  @doc "Names bound right now (for introspection / api surface)."
  @spec names() :: [atom()]
  def names do
    {binding, _env} = snapshot()
    binding |> Keyword.keys() |> Enum.sort()
  end

  @doc "Drop all bindings and start from the prelude env again."
  @spec reset() :: :ok
  def reset do
    GenServer.call(__MODULE__, :reset)
  end

  @impl true
  def init(_) do
    case IxMcp.Checkpoint.fetch() do
      {:ok, binding, env} -> {:ok, %{binding: binding, env: env}}
      :empty -> {:ok, fresh()}
    end
  end

  @impl true
  def handle_call(:snapshot, _from, state) do
    {:reply, {state.binding, state.env}, state}
  end

  def handle_call({:merge, binding, env}, _from, state) do
    # Variables the cell bound or rebound win; variables it never touched
    # (pruned from its returned binding) keep their current values.
    merged = Keyword.merge(state.binding, binding)
    state = %{state | binding: merged, env: env}
    IxMcp.Checkpoint.store(state.binding, state.env)
    {:reply, :ok, state}
  end

  def handle_call(:reset, _from, _state) do
    state = fresh()
    IxMcp.Checkpoint.store(state.binding, state.env)
    {:reply, :ok, state}
  end

  defp fresh do
    env = Code.env_for_eval(file: "cell")
    # Evaluate the prelude so its aliases live in the env every cell sees;
    # cells can then write `Jobs.tail("ab12", 20)` with no setup.
    {_value, binding, env} = Code.eval_quoted_with_env(quoted_prelude(), [], env)
    %{binding: binding, env: env}
  end

  defp quoted_prelude do
    Code.string_to_quoted!(@prelude, file: "prelude")
  end
end
