defmodule FleetMesh.Engine do
  @moduledoc """
  Evaluates a policy's conditions on their intervals and tells consumers two
  things: the whole picture once, and every change after.

  `snapshot/1` is the whole picture: the current `{state, since, detail}` per
  condition, for surfaces that say "here is where the fleet stands right
  now" (a session's first message, a page load). `subscribe/2` is the
  changes: the caller immediately receives one `{:fleet_snapshot, states}`
  message so it starts from the same picture, and after that only
  `{:fleet_edge, id, from, to, detail}` on transitions. Steady state is
  silent by design; a subscriber that wants a heartbeat can read the
  snapshot on its own clock.

  Subscribing reports back who already subscribed, so a caller can decline
  to duplicate a watch: notification fan-out is meant to be opt-in and rare,
  and the second would-be watcher deciding "session N already has this" is
  how it stays that way.

  A check that stops producing answers is a state of its own, `:unknown`,
  and it edges like any other transition (see `FleetMesh.Condition`): an
  engine whose reads are failing must not be mistaken for an engine
  reporting green.

  The first evaluation is scheduled immediately after init rather than run
  inside it: a check can take a network round trip, and a host application
  must not hang its boot on one. Until it completes, `snapshot/1` returns
  `%{}`; a consumer that must distinguish "not evaluated yet" from "no
  conditions" renders the empty map as still-evaluating. `refresh/2` is the
  synchronous form: evaluate everything now and return the result, for
  callers whose question is "what is true right now", not "what did the last
  tick see".
  """

  use GenServer

  alias FleetMesh.Condition
  alias FleetMesh.Policy

  @typedoc "What the engine knows about one condition right now."
  @type entry :: %{state: Condition.state(), since: integer(), detail: term()}

  @typedoc "Everything the engine knows, keyed by condition id."
  @type states :: %{atom() => entry()}

  # -- client ---------------------------------------------------------------

  @doc """
  Start the engine. Options:

    * `:policy` -- a `FleetMesh.Policy` module. Defaults to
      `FleetMesh.Policy.configured!/0`, which raises when unconfigured.
    * `:name` -- GenServer registration, default `#{inspect(__MODULE__)}`.
  """
  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    {name, opts} = Keyword.pop(opts, :name, __MODULE__)
    GenServer.start_link(__MODULE__, opts, name: name)
  end

  @doc "The current state of every condition."
  @spec snapshot(GenServer.server()) :: states()
  def snapshot(server \\ __MODULE__), do: GenServer.call(server, :snapshot)

  @doc """
  Subscribe the calling process to edges.

  The caller is sent `{:fleet_snapshot, states}` immediately, then
  `{:fleet_edge, id, from, to, detail}` per transition. Returns
  `{:ok, already}` where `already` lists the `info` values of every other
  live subscriber; a caller seeing a non-empty list can decline to watch
  twice. The subscription dies with the caller (monitored).
  """
  @spec subscribe(GenServer.server(), term()) :: {:ok, [term()]}
  def subscribe(server \\ __MODULE__, info \\ %{}) do
    GenServer.call(server, {:subscribe, self(), info})
  end

  @doc "Remove the calling process's subscription."
  @spec unsubscribe(GenServer.server()) :: :ok
  def unsubscribe(server \\ __MODULE__), do: GenServer.call(server, {:unsubscribe, self()})

  # -- server ---------------------------------------------------------------

  @impl true
  def init(opts) do
    policy = Keyword.get_lazy(opts, :policy, &Policy.configured!/0)

    conditions =
      for condition <- policy.conditions() do
        # A policy handing over anything but the struct fails here, at boot,
        # not at first evaluation.
        %Condition{} = condition
      end

    state = %{
      conditions: Map.new(conditions, &{&1.id, &1}),
      states: %{},
      subscribers: %{}
    }

    if map_size(state.conditions) != length(conditions) do
      raise ArgumentError,
            "policy #{inspect(policy)} declares duplicate condition ids: " <>
              inspect(Enum.map(conditions, & &1.id))
    end

    {:ok, state, {:continue, :first_evaluation}}
  end

  @impl true
  def handle_continue(:first_evaluation, state) do
    {:noreply, Enum.reduce(Map.values(state.conditions), state, &evaluate_and_schedule/2)}
  end

  @doc """
  Evaluate every condition now, synchronously, and return the resulting
  snapshot. Edges fire exactly as on a scheduled evaluation. The scheduled
  timers are untouched, so a refresh brings the next tick's answer forward
  rather than replacing it. Checks run in the engine process; pass a
  `timeout` sized to the slowest check.
  """
  @spec refresh(GenServer.server(), timeout()) :: states()
  def refresh(server \\ __MODULE__, timeout \\ :infinity) do
    GenServer.call(server, :refresh, timeout)
  end

  @impl true
  def handle_call(:snapshot, _from, state), do: {:reply, state.states, state}

  def handle_call(:refresh, _from, state) do
    refreshed = Enum.reduce(Map.values(state.conditions), state, &evaluate_only/2)
    {:reply, refreshed.states, refreshed}
  end

  def handle_call({:subscribe, pid, info}, _from, state) do
    already = for {other, %{info: i}} <- state.subscribers, other != pid, do: i
    ref = Process.monitor(pid)
    send(pid, {:fleet_snapshot, state.states})
    {:reply, {:ok, already}, put_in(state.subscribers[pid], %{info: info, ref: ref})}
  end

  def handle_call({:unsubscribe, pid}, _from, state) do
    {:reply, :ok, drop_subscriber(state, pid)}
  end

  @impl true
  def handle_info({:evaluate, id}, state) do
    case Map.fetch(state.conditions, id) do
      {:ok, condition} -> {:noreply, evaluate_and_schedule(condition, state)}
      :error -> {:noreply, state}
    end
  end

  def handle_info({:DOWN, _ref, :process, pid, _reason}, state) do
    {:noreply, drop_subscriber(state, pid)}
  end

  # -- internals ------------------------------------------------------------

  @spec evaluate_and_schedule(Condition.t(), map()) :: map()
  defp evaluate_and_schedule(condition, state) do
    Process.send_after(self(), {:evaluate, condition.id}, condition.interval_ms)
    evaluate_only(condition, state)
  end

  @spec evaluate_only(Condition.t(), map()) :: map()
  defp evaluate_only(condition, state) do
    {next, detail} = Condition.evaluate(condition)

    case Map.get(state.states, condition.id) do
      # First evaluation: establish the state, no edge. The snapshot carries
      # it; an edge with no prior state would make every boot look like a
      # fleet-wide transition.
      nil ->
        put_entry(state, condition.id, next, detail)

      %{state: ^next} ->
        # Same state: refresh the detail, keep `since`. Silent by design.
        put_in(state.states[condition.id].detail, detail)

      %{state: prior} ->
        Enum.each(Map.keys(state.subscribers), fn pid ->
          send(pid, {:fleet_edge, condition.id, prior, next, detail})
        end)

        put_entry(state, condition.id, next, detail)
    end
  end

  @spec put_entry(map(), atom(), Condition.state(), term()) :: map()
  defp put_entry(state, id, condition_state, detail) do
    entry = %{state: condition_state, since: System.system_time(:second), detail: detail}
    put_in(state.states[id], entry)
  end

  @spec drop_subscriber(map(), pid()) :: map()
  defp drop_subscriber(state, pid) do
    case Map.pop(state.subscribers, pid) do
      {nil, _} ->
        state

      {%{ref: ref}, rest} ->
        Process.demonitor(ref, [:flush])
        %{state | subscribers: rest}
    end
  end
end
