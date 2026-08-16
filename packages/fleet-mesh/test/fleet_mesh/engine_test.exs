defmodule FleetMesh.EngineTest do
  use ExUnit.Case, async: true

  alias FleetMesh.Condition
  alias FleetMesh.Engine

  # A condition whose answer is whatever the test last put in the agent:
  # flipping the agent is flipping the fleet.
  defp flippable(agent) do
    Condition.new(
      id: :flip,
      severity: :warning,
      description: "flips on demand",
      interval_ms: 10,
      check: fn -> Agent.get(agent, & &1) end
    )
  end

  defp start_engine(context, conditions) do
    policy = %{conditions: conditions}

    start_supervised!(
      {Engine, policy: test_policy(policy), name: :"engine_#{context.test}"},
      id: :engine
    )

    :"engine_#{context.test}"
  end

  # A throwaway policy module per test. Conditions hold closures, which
  # cannot be escaped into module AST, so they travel via :persistent_term
  # and the generated module only embeds the (escapable) key.
  defp test_policy(%{conditions: conditions}) do
    key = {__MODULE__, System.unique_integer([:positive])}
    :persistent_term.put(key, conditions)

    {:module, mod, _, _} =
      Module.create(
        Module.concat(__MODULE__, :"Policy#{System.unique_integer([:positive])}"),
        quote do
          @behaviour FleetMesh.Policy
          @impl true
          def conditions, do: :persistent_term.get(unquote(Macro.escape(key)))
        end,
        Macro.Env.location(__ENV__)
      )

    mod
  end

  test "snapshot carries the first evaluation, no edge for it", context do
    {:ok, agent} = Agent.start_link(fn -> {:red, :born_red} end)
    server = start_engine(context, [flippable(agent)])

    assert %{flip: %{state: :red, detail: :born_red}} = Engine.snapshot(server)

    {:ok, _} = Engine.subscribe(server)
    assert_receive {:fleet_snapshot, %{flip: %{state: :red}}}
    refute_receive {:fleet_edge, _, _, _, _}, 50
  end

  test "a transition reaches subscribers as one edge, steady state is silent",
       context do
    {:ok, agent} = Agent.start_link(fn -> :green end)
    server = start_engine(context, [flippable(agent)])
    {:ok, _} = Engine.subscribe(server)
    assert_receive {:fleet_snapshot, _}

    Agent.update(agent, fn _ -> {:red, :now_red} end)
    assert_receive {:fleet_edge, :flip, :green, :red, :now_red}, 500
    refute_receive {:fleet_edge, _, _, _, _}, 50

    Agent.update(agent, fn _ -> :green end)
    assert_receive {:fleet_edge, :flip, :red, :green, nil}, 500
  end

  test "a check that starts raising edges into :unknown", context do
    {:ok, agent} = Agent.start_link(fn -> :green end)
    server = start_engine(context, [flippable(agent)])
    {:ok, _} = Engine.subscribe(server)
    assert_receive {:fleet_snapshot, _}

    Agent.update(agent, fn _ -> nil end)
    # nil is not a state, so evaluate reports the unexpected shape.
    assert_receive {:fleet_edge, :flip, :green, :unknown, {:unexpected_check_return, nil}}, 500
  end

  test "subscribe reports who is already watching, and only while they live", context do
    server = start_engine(context, [])
    parent = self()

    watcher =
      spawn_link(fn ->
        {:ok, already} = Engine.subscribe(server, %{session: 7})
        send(parent, {:first_saw, already})

        receive do
          :stop -> :ok
        end
      end)

    assert_receive {:first_saw, []}
    assert {:ok, [%{session: 7}]} = Engine.subscribe(server, %{session: 9})

    # A dead watcher stops counting: the report lists live sessions only.
    send(watcher, :stop)
    Process.sleep(20)
    assert {:ok, already} = Engine.subscribe(server, %{session: 11})
    refute Enum.member?(already, %{session: 7})
  end

  test "a dead subscriber is dropped, not crashed on", context do
    {:ok, agent} = Agent.start_link(fn -> :green end)
    server = start_engine(context, [flippable(agent)])

    {:ok, watcher} =
      Task.start(fn ->
        {:ok, _} = Engine.subscribe(server)

        receive do
          :never -> :ok
        end
      end)

    # Let the subscription land, then kill the watcher and flip: the engine
    # must survive sending an edge to nobody.
    Process.sleep(20)
    Process.exit(watcher, :kill)
    Agent.update(agent, fn _ -> :red end)
    Process.sleep(50)
    assert %{flip: %{state: :red}} = Engine.snapshot(server)
  end

  test "refresh evaluates now and returns the new picture", context do
    {:ok, agent} = Agent.start_link(fn -> :green end)
    server = start_engine(context, [flippable(agent)])
    {:ok, _} = Engine.subscribe(server)
    assert_receive {:fleet_snapshot, _}

    Agent.update(agent, fn _ -> {:red, :seen_by_refresh} end)
    assert %{flip: %{state: :red, detail: :seen_by_refresh}} = Engine.refresh(server)
    # The refresh emitted the edge exactly as a scheduled evaluation would.
    assert_receive {:fleet_edge, :flip, :green, :red, :seen_by_refresh}
  end

  test "an unconfigured policy fails at start, loudly" do
    Process.flag(:trap_exit, true)
    assert {:error, {%ArgumentError{}, _stack}} = Engine.start_link(name: :unconfigured_engine)
  end

  test "duplicate condition ids are refused at start", context do
    {:ok, agent} = Agent.start_link(fn -> :green end)
    Process.flag(:trap_exit, true)

    assert {:error, {%ArgumentError{message: message}, _stack}} =
             Engine.start_link(
               policy: test_policy(%{conditions: [flippable(agent), flippable(agent)]}),
               name: :"dup_#{context.test}"
             )

    assert message =~ "duplicate condition ids"
  end
end
