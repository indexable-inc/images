defmodule SymphonyElixir.Runtime.OperatorControlsTest do
  @moduledoc """
  The #97 operator surface: cancel, retry, rerun, and clear-failed, each
  recording a durable audit event. Driven against a fake engine so a node
  can be made to fail on demand.
  """
  use ExUnit.Case, async: false

  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Graph
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.IR.Store
  alias SymphonyElixir.Runtime

  @moduletag capture_log: true

  defmodule FakeEngine do
    @moduledoc false
    @behaviour SymphonyElixir.Runtime.EngineClient

    @table :operator_controls_fake

    def setup do
      if :ets.whereis(@table) == :undefined, do: :ets.new(@table, [:named_table, :public, :set])
      :ets.delete_all_objects(@table)
      :ok
    end

    def program(node_id, instruction), do: :ets.insert(@table, {node_id, instruction})

    @impl true
    def run_node(%Node{id: id}, _opts) do
      case :ets.lookup(@table, id) do
        [{^id, {:error, reason}}] -> {:error, reason, nil}
        [{^id, {:ok, output}}] -> {:ok, output, "thread-#{id}"}
        [] -> {:ok, %{ran: id}, "thread-#{id}"}
      end
    end

    @impl true
    def status(_thread_id), do: :unknown
  end

  setup do
    FakeEngine.setup()
    start_supervised!({Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry})
    start_supervised!({Task.Supervisor, name: SymphonyElixir.TaskSupervisor})

    tmp = Path.join(System.tmp_dir!(), "op_ctrl_#{System.unique_integer([:positive])}")
    File.mkdir_p!(tmp)
    on_exit(fn -> File.rm_rf(tmp) end)
    {:ok, store_opts: [dir: tmp]}
  end

  # Agent nodes so each attempt routes through the injected engine. The
  # operator surface is engine-agnostic; an agent node is the kind that
  # actually consults the EngineClient (exec nodes run locally).
  defp agent_node(id, inputs \\ %{}) do
    Node.new(
      id: id,
      ast_origin: {:agent, id},
      kind: :agent,
      envelope: %Envelope{engine: :codex, model: "m"},
      prompt_ref: {:inline, "go"},
      inputs: inputs,
      state: :pending
    )
  end

  # Two-node chain a -> b, where b reads a's output so b only runs after a.
  defp chain_graph(run_id) do
    a = agent_node("a")
    b = agent_node("b", %{"x" => {:node, "a", []}})
    run_id |> RunGraph.new("hash", nil) |> RunGraph.put_nodes([a, b]) |> Map.put(:status, :running)
  end

  defp wait_terminal(run_id, store_opts, attempts \\ 40) do
    {:ok, graph} = Store.load(run_id, store_opts)

    cond do
      graph.status in [:succeeded, :failed, :cancelled] -> graph
      attempts == 0 -> flunk("run #{run_id} never terminal: #{graph.status}")
      true -> Process.sleep(25) && wait_terminal(run_id, store_opts, attempts - 1)
    end
  end

  test "clear_failed resets failed nodes and the rerun succeeds", %{store_opts: store_opts} do
    FakeEngine.program("a", {:error, :boom})
    graph = chain_graph("run_clear")

    {:ok, pid} = Runtime.start_link(graph, engine: FakeEngine, store_opts: store_opts)

    # Wait for the run to fail (a fails, b becomes upstream_failed).
    failed = wait_for(pid, fn g -> Graph.all_terminal?(g) end)
    assert failed.nodes["a"].state == :failed
    assert failed.nodes["b"].state == :upstream_failed

    # Fix the cause, then clear the failed nodes. They re-run and succeed,
    # at which point the run reaches a terminal :succeeded and the GenServer
    # stops, so read the recovered state from the store.
    FakeEngine.program("a", {:ok, %{fixed: true}})
    ref = Process.monitor(pid)
    :ok = Runtime.clear_failed(pid, "alice")
    assert_receive {:DOWN, ^ref, :process, _, _}, 2_000

    recovered = wait_terminal("run_clear", store_opts)
    assert recovered.status == :succeeded
    assert recovered.nodes["a"].state == :succeeded
    assert recovered.nodes["b"].state == :succeeded

    # The clear_failed action is recorded with the actor and the cleared ids.
    event = Enum.find(recovered.audit_log, &(&1.action == :clear_failed))
    assert event.actor == "alice"
    assert Enum.sort(event.detail.cleared) == ["a", "b"]
  end

  test "cancel records an audit event and stops the run", %{store_opts: store_opts} do
    # Keep a node busy so the run is still in flight when we cancel.
    FakeEngine.program("a", {:ok, %{}})
    graph = chain_graph("run_cancel")
    {:ok, pid} = Runtime.start_link(graph, engine: FakeEngine, store_opts: store_opts)
    ref = Process.monitor(pid)

    :ok = Runtime.cancel(pid, "bob")
    assert_receive {:DOWN, ^ref, :process, _, _}, 2_000

    {:ok, final} = Store.load("run_cancel", store_opts)
    assert final.status == :cancelled
    event = Enum.find(final.audit_log, &(&1.action == :cancel))
    assert event.actor == "bob"
  end

  test "retry_node re-runs only the target node and records the audit event", %{store_opts: store_opts} do
    # A single independent node so the surgical retry can drive the run to a
    # clean terminal without an upstream_failed dependent lingering.
    node = agent_node("a")
    graph = "run_retry" |> RunGraph.new("hash", nil) |> RunGraph.put_nodes([node]) |> Map.put(:status, :running)

    FakeEngine.program("a", {:error, :nope})
    {:ok, pid} = Runtime.start_link(graph, engine: FakeEngine, store_opts: store_opts)

    wait_for(pid, fn g -> g.nodes["a"].state == :failed end)
    FakeEngine.program("a", {:ok, %{}})
    ref = Process.monitor(pid)
    :ok = Runtime.retry_node(pid, "a", "carol")
    assert_receive {:DOWN, ^ref, :process, _, _}, 2_000

    final = wait_terminal("run_retry", store_opts)
    assert final.nodes["a"].state == :succeeded
    event = Enum.find(final.audit_log, &(&1.action == :retry_node))
    assert event.target == "a"
    assert event.actor == "carol"
  end

  test "audit log survives a store round-trip", %{store_opts: store_opts} do
    graph =
      "run_audit_rt"
      |> chain_graph()
      |> RunGraph.append_audit(:clear_failed, nil, "dave", %{cleared: ["a"]})
      |> RunGraph.append_audit(:cancel, "b", :system, %{})

    :ok = Store.persist(graph, store_opts)
    {:ok, loaded} = Store.load("run_audit_rt", store_opts)

    assert [first, second] = loaded.audit_log
    assert first.action == :clear_failed
    assert first.actor == "dave"
    assert first.detail == %{cleared: ["a"]}
    assert second.action == :cancel
    assert second.target == "b"
    assert second.actor == :system
  end

  # Poll the live runtime's graph snapshot until `pred` holds.
  defp wait_for(pid, pred, attempts \\ 80) do
    graph = Runtime.graph(pid)

    cond do
      pred.(graph) -> graph
      attempts == 0 -> flunk("condition never held; last status=#{graph.status}")
      true -> Process.sleep(20) && wait_for(pid, pred, attempts - 1)
    end
  end
end
