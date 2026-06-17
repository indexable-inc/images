defmodule Hive.Agent do
  @moduledoc """
  One actor (a GenServer process) per agent.

  Agents address each other by a logical `id`, which `Hive.Registry` resolves to
  the agent's live pid at send time. Nobody holds raw pids, so the mesh is fully
  connected by construction: any agent can reach any other just by knowing its id.

  ## Types

  This module leans on Elixir 1.18's set-theoretic type checker. The payoff lives
  in `State` below: because the per-agent state is a typed struct rather than a
  bare map, the compiler knows the exact set of fields and their types, so every
  `state.inbox` access and `%State{state | ...}` update is checked at compile time.
  The `when is_atom(id)` guards narrow each `id` to the `atom()` set on entry, and
  the `@spec`s pin every boundary to a precise type (atom singletons in the `:via`
  tuple, tagged-tuple `envelope/0`, union return types like `GenServer.on_start/0`).
  """

  use GenServer

  @typedoc "A logical agent name. Ids are atoms, so they read as `:planner`, `:critic`, …"
  @type id :: atom()

  @typedoc "Anything an agent can carry as a message body."
  @type payload :: term()

  @typedoc "A received message: who sent it (return address) and what they said."
  @type envelope :: {id(), payload()}

  @typedoc "The wire format of a peer message, as handled by `handle_cast/2`."
  @type request :: {:message, id(), payload()}

  defmodule State do
    @moduledoc """
    Per-agent process state: the agent's own `id` and its received `inbox`
    (newest first; reversed to oldest-first on read).

    A typed struct, not a map, on purpose. With `@enforce_keys` plus the `@type t`
    below, Elixir 1.18's checker knows `id` is always present and an `atom()`, and
    that `inbox` is a list of `envelope/0`s. A typo like `state.inboxx` or storing
    the wrong shape in `inbox` is then a compile-time error, not a runtime surprise.
    """

    @enforce_keys [:id]
    defstruct [:id, inbox: []]

    @type t :: %__MODULE__{
            id: Hive.Agent.id(),
            inbox: [Hive.Agent.envelope()]
          }
  end

  # ---- client API: these run in the CALLER's process, not the agent's ----

  @doc "Start an agent registered under `id`. Invoked by the DynamicSupervisor."
  @spec start_link(id()) :: GenServer.on_start()
  def start_link(id) when is_atom(id) do
    GenServer.start_link(__MODULE__, id, name: via(id))
  end

  @doc """
  Send `payload` to one agent, tagged with the sender's `from_id`.

  Async (`cast`) on purpose: in a fully connected graph, synchronous calls
  between agents can form a cycle and deadlock (each GenServer handles one
  message at a time). Fire-and-forget can't. Replies are just another whisper.
  """
  @spec whisper(id(), id(), payload()) :: :ok
  def whisper(target_id, from_id, payload) when is_atom(target_id) and is_atom(from_id) do
    GenServer.cast(via(target_id), {:message, from_id, payload})
  end

  @doc "Send `payload` to every other live agent."
  @spec broadcast(id(), payload()) :: :ok
  def broadcast(from_id, payload) when is_atom(from_id) do
    for id <- ids(), id != from_id, do: whisper(id, from_id, payload)
    :ok
  end

  @doc "Read an agent's received messages, oldest first (sync call from outside)."
  @spec inbox(id()) :: [envelope()]
  def inbox(id) when is_atom(id), do: GenServer.call(via(id), :inbox)

  @doc "Every currently live agent id (the registry is the source of truth)."
  @spec ids() :: [id()]
  def ids do
    Registry.select(Hive.Registry, [{{:"$1", :_, :_}, [], [:"$1"]}])
  end

  # The :via tuple is usable anywhere a process name is expected. It means
  # "look `id` up in Hive.Registry and deliver to whatever pid is registered."
  # The spec is a tuple of atom singletons (`:via`, the `Registry`/`Hive.Registry`
  # module atoms): a small but exact set-theoretic type.
  @spec via(id()) :: {:via, Registry, {Hive.Registry, id()}}
  defp via(id), do: {:via, Registry, {Hive.Registry, id}}

  # ---- server callbacks: these run INSIDE the agent process ----

  @impl true
  @spec init(id()) :: {:ok, State.t()}
  def init(id), do: {:ok, %State{id: id}}

  @impl true
  @spec handle_cast(request(), State.t()) :: {:noreply, State.t()}
  def handle_cast({:message, from_id, payload}, %State{} = state) do
    IO.puts("[#{state.id}] <- #{from_id}: #{inspect(payload)}")
    {:noreply, %State{state | inbox: [{from_id, payload} | state.inbox]}}
  end

  @impl true
  @spec handle_call(:inbox, GenServer.from(), State.t()) :: {:reply, [envelope()], State.t()}
  def handle_call(:inbox, _from, %State{} = state) do
    {:reply, Enum.reverse(state.inbox), state}
  end
end
