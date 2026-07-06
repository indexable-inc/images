defmodule SymphonyElixir.Runtime.SupervisorTest do
  use ExUnit.Case, async: false

  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.IR.Store
  alias SymphonyElixir.Runtime

  @moduletag capture_log: true

  defmodule FakeEngine do
    @moduledoc false
    @behaviour SymphonyElixir.Runtime.EngineClient

    @impl true
    def run_node(%Node{id: id}, _opts), do: {:ok, %{ran: id}, "thread-#{id}"}

    @impl true
    def status(_thread_id), do: :unknown
  end

  setup do
    start_supervised!({Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry})
    start_supervised!({Task.Supervisor, name: SymphonyElixir.TaskSupervisor})
    start_supervised!(SymphonyElixir.Runtime.Supervisor)

    tmp = Path.join(System.tmp_dir!(), "rt_sup_#{System.unique_integer([:positive])}")
    File.mkdir_p!(tmp)
    on_exit(fn -> File.rm_rf(tmp) end)
    {:ok, store_opts: [dir: tmp]}
  end

  # Agent nodes route through the injected FakeEngine; exec nodes run
  # locally and would bypass it.
  defp agent_node(id, overrides \\ []) do
    Node.new(
      [
        id: id,
        ast_origin: {:agent, id},
        kind: :agent,
        envelope: %Envelope{engine: :codex, model: "m"},
        prompt_ref: {:inline, "go"},
        inputs: %{}
      ] ++ overrides
    )
  end

  defp one_node_graph(run_id) do
    node = agent_node("n0")
    run_id |> RunGraph.new("hash", nil) |> RunGraph.put_nodes([node]) |> Map.put(:status, :running)
  end

  test "start_run schedules a graph under supervision and it runs to terminal", %{store_opts: store_opts} do
    graph = one_node_graph("run_sup_1")

    assert {:ok, pid} = Runtime.Supervisor.start_run(graph, engine: FakeEngine, store_opts: store_opts)
    ref = Process.monitor(pid)
    assert_receive {:DOWN, ^ref, :process, _, _}, 2_000

    {:ok, final} = Store.load("run_sup_1", store_opts)
    assert final.status == :succeeded
    assert final.nodes["n0"].state == :succeeded
  end

  test "resume_pending restarts a persisted non-terminal run with recovery", %{store_opts: store_opts} do
    # Persist a run left :running with a node :running (an orphaned run, as
    # if the BEAM died mid-flight). resume_pending should reattach/recover.
    node = agent_node("n0", state: :running)
    graph = "run_resume" |> RunGraph.new("hash", nil) |> RunGraph.put_nodes([node]) |> Map.put(:status, :running)
    :ok = Store.persist(graph, store_opts)

    Runtime.Supervisor.resume_pending(engine: FakeEngine, store_opts: store_opts)

    # The recovered run reconciles the orphaned :running node. With a
    # FakeEngine status of :unknown the node is stranded (no opt-in retry),
    # so the run resolves rather than hanging. Poll the store until terminal.
    final = wait_for_terminal("run_resume", store_opts)
    assert final.status in [:failed, :succeeded]
    refute final.nodes["n0"].state == :running
  end

  test "resume_pending skips terminal runs", %{store_opts: store_opts} do
    node = agent_node("n0", state: :succeeded)
    graph = "run_done" |> RunGraph.new("hash", nil) |> RunGraph.put_nodes([node]) |> Map.put(:status, :succeeded)
    :ok = Store.persist(graph, store_opts)

    Runtime.Supervisor.resume_pending(engine: FakeEngine, store_opts: store_opts)

    # No child was started for the already-terminal run.
    assert DynamicSupervisor.count_children(SymphonyElixir.Runtime.Supervisor).active == 0
  end

  defp wait_for_terminal(run_id, store_opts, attempts \\ 40) do
    {:ok, graph} = Store.load(run_id, store_opts)

    cond do
      graph.status in [:succeeded, :failed, :cancelled] -> graph
      attempts == 0 -> flunk("run #{run_id} never reached terminal: #{graph.status}")
      true -> Process.sleep(25) && wait_for_terminal(run_id, store_opts, attempts - 1)
    end
  end
end
