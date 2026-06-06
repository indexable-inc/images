defmodule SymphonyElixir.Runtime.DSLWiringTest do
  @moduledoc """
  End-to-end proof that a parsed `.sym` workflow drives the IR runtime:
  Parser -> Materializer -> Runtime -> a fake engine -> terminal nodes,
  including the dynamic expansion of a `when` gate after its dependency
  succeeds. This is the WS-5 seam (interpreter <-> runtime) under test
  against a fake `EngineClient`, so no room-server is required.
  """
  use ExUnit.Case, async: false

  @moduletag capture_log: true

  alias SymphonyElixir.DSL.Parser
  alias SymphonyElixir.IR.{Materializer, Node, Store}
  alias SymphonyElixir.Runtime

  # A fake engine that returns a per-node-id scripted output. The gate's
  # dependency returns `%{"ok" => true}` so the gate opens; every other
  # node returns a trivial success.
  defmodule FakeEngine do
    @behaviour SymphonyElixir.Runtime.EngineClient

    @table :dsl_wiring_fake

    def setup do
      if :ets.whereis(@table) == :undefined, do: :ets.new(@table, [:named_table, :public, :set])
      :ets.delete_all_objects(@table)
      :ok
    end

    def program(node_id, output), do: :ets.insert(@table, {node_id, output})

    @impl true
    def run_node(%Node{id: id}, _opts) do
      case :ets.lookup(@table, id) do
        [{^id, output}] -> {:ok, output, "thread-#{id}"}
        [] -> {:ok, %{default: id}, "thread-#{id}"}
      end
    end

    @impl true
    def status(_thread_id), do: :unknown
  end

  setup do
    FakeEngine.setup()
    start_supervised!({Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry})
    start_supervised!({Task.Supervisor, name: SymphonyElixir.TaskSupervisor})
    tmp = Path.join(System.tmp_dir!(), "dsl_wiring_#{System.unique_integer([:positive])}")
    File.mkdir_p!(tmp)
    on_exit(fn -> File.rm_rf(tmp) end)
    {:ok, store_opts: [dir: tmp]}
  end

  defp materialize!(source, run_id) do
    {:ok, ast} = Parser.parse(source)
    {:ok, graph} = Materializer.materialize(run_id, "hash-#{run_id}", ast)
    graph
  end

  test "a two-node linear workflow runs both nodes to succeeded", %{store_opts: store_opts} do
    source = """
    workflow "w" {
      a <- agent { engine: codex, model: "m", prompt: inline "first" }
      b <- agent { engine: codex, model: "m", prompt: skill "next" { ctx: ${a.area} } }
    }
    """

    graph = materialize!(source, "run_lin")
    FakeEngine.program("agent-0", %{"area" => 7})

    {:ok, pid} = Runtime.start_link(graph, engine: FakeEngine, store_opts: store_opts)
    ref = Process.monitor(pid)
    assert_receive {:DOWN, ^ref, :process, _, _}, 2_000

    # Read the persisted final graph from the store.
    {:ok, final} = Store.load("run_lin", store_opts)
    assert final.status == :succeeded
    assert final.nodes["agent-0"].state == :succeeded
    assert final.nodes["agent-1"].state == :succeeded
    # The edge held: agent-1 only ran after agent-0 succeeded.
    assert "agent-0" in final.nodes["agent-1"].deps
  end

  test "a when-gate expands and runs its body after the dependency succeeds", %{store_opts: store_opts} do
    source = """
    workflow "w" {
      a <- agent { engine: codex, model: "m", prompt: inline "first" }
      when ${a.ok} {
        b <- agent { engine: codex, model: "m", prompt: inline "gated" }
      }
    }
    """

    graph = materialize!(source, "run_gate")
    FakeEngine.program("agent-0", %{"ok" => true})

    {:ok, pid} = Runtime.start_link(graph, engine: FakeEngine, store_opts: store_opts)
    ref = Process.monitor(pid)
    assert_receive {:DOWN, ^ref, :process, _, _}, 2_000

    {:ok, final} = Store.load("run_gate", store_opts)
    assert final.status == :succeeded
    assert final.nodes["agent-0"].state == :succeeded

    # The gated body node was emitted dynamically and ran to success.
    body = Enum.find(Map.values(final.nodes), fn n -> n.kind == :agent and n.id != "agent-0" end)
    assert body, "gate body node was never materialized"
    assert body.state == :succeeded

    # The gate placeholder was retired, not left pending.
    gate = Enum.find(Map.values(final.nodes), &(&1.kind == :gate))
    assert gate.state == :skipped
  end

  test "a falsey when-gate skips the body and the run still succeeds", %{store_opts: store_opts} do
    source = """
    workflow "w" {
      a <- agent { engine: codex, model: "m", prompt: inline "first" }
      when ${a.ok} {
        b <- agent { engine: codex, model: "m", prompt: inline "gated" }
      }
    }
    """

    graph = materialize!(source, "run_skip")
    FakeEngine.program("agent-0", %{"ok" => false})

    {:ok, pid} = Runtime.start_link(graph, engine: FakeEngine, store_opts: store_opts)
    ref = Process.monitor(pid)
    assert_receive {:DOWN, ^ref, :process, _, _}, 2_000

    {:ok, final} = Store.load("run_skip", store_opts)
    assert final.status == :succeeded
    assert final.nodes["agent-0"].state == :succeeded
    # No body agent node was emitted.
    refute Enum.any?(Map.values(final.nodes), fn n -> n.kind == :agent and n.id != "agent-0" end)
    # The gate placeholder was retired to :skipped.
    assert Enum.find(Map.values(final.nodes), &(&1.kind == :gate)).state == :skipped
  end
end
