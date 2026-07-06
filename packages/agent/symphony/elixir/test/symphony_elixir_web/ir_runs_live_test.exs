defmodule SymphonyElixirWeb.IRRunsLiveTest do
  @moduledoc """
  Phase 5 tests: the :show LiveView renders the graph SVG, the summary dl,
  and action buttons that drive Runtime operator calls.
  """

  use ExUnit.Case, async: false

  import Phoenix.ConnTest
  import Phoenix.LiveViewTest

  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.IR.Store
  alias SymphonyElixir.Runtime

  @endpoint SymphonyElixirWeb.Endpoint

  # A fake EngineClient that blocks indefinitely (sleep_forever) so the run
  # stays :running while the test exercises operator actions. Using async:
  # false and a named table so concurrent suites do not interfere.
  defmodule FakeEngine do
    @moduledoc false
    @behaviour SymphonyElixir.Runtime.EngineClient

    @table :ir_runs_live_fake

    def setup do
      if :ets.whereis(@table) == :undefined do
        :ets.new(@table, [:named_table, :public, :set])
      end

      :ets.delete_all_objects(@table)
      :ok
    end

    def program(node_id, instruction), do: :ets.insert(@table, {node_id, instruction})

    @impl true
    def run_node(%Node{id: id}, _opts) do
      case :ets.lookup(@table, id) do
        [{^id, :block}] ->
          # Block until the test is done by sleeping a long time. The task
          # will be killed when the runtime stops.
          Process.sleep(30_000)
          {:ok, %{}, nil}

        [{^id, {:ok, out}}] ->
          {:ok, out, nil}

        [{^id, {:error, reason}}] ->
          {:error, reason, nil}

        [] ->
          {:ok, %{default: id}, nil}
      end
    end

    @impl true
    def status(_thread_id), do: :unknown
  end

  setup do
    FakeEngine.setup()

    if !Process.whereis(SymphonyElixir.Runtime.Registry) do
      start_supervised!({Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry})
    end

    if !Process.whereis(SymphonyElixir.TaskSupervisor) do
      start_supervised!({Task.Supervisor, name: SymphonyElixir.TaskSupervisor})
    end

    # The LiveView mount calls WorkflowCatalog.workflows/0 and errors/0,
    # which read two ETS tables. Create both if not present, mirroring the
    # pattern used in IRRunControllerTest.
    for table <- [:symphony_workflows, :symphony_workflow_errors] do
      if :ets.whereis(table) == :undefined do
        :ets.new(table, [:named_table, :public, read_concurrency: true])
      else
        :ets.delete_all_objects(table)
      end
    end

    :ok
  end

  defp agent_node(id, opts \\ []) do
    Node.new(
      id: id,
      ast_origin: {:agent, id},
      kind: :agent,
      envelope: %Envelope{engine: :codex, model: "gpt-5.3-codex"},
      inputs: Keyword.get(opts, :inputs, %{}),
      state: :pending
    )
  end

  defp persist_graph(graph, store_opts \\ []) do
    :ok = Store.persist(graph, store_opts)
  end

  defp build_graph(run_id, nodes) do
    run_id
    |> RunGraph.new("hash", nil)
    |> RunGraph.put_nodes(nodes)
    |> Map.put(:status, :running)
  end

  test "show page renders the summary dl with trigger and placement" do
    run_id = "live-show-#{System.unique_integer([:positive])}"

    graph =
      run_id
      |> build_graph([agent_node("a"), agent_node("b", inputs: %{"x" => {:node, "a", []}})])
      |> Map.put(:trigger, %{kind: :manual})

    persist_graph(graph)

    {:ok, view, html} = live(build_conn(), "/ir/" <> run_id)

    # The summary dl should be present.
    assert html =~ "<dl"
    assert html =~ "kv"
    # Trigger is shown.
    assert html =~ "trigger"
    assert html =~ "manual"
    # Placement label is shown (nil placement renders as "-").
    assert html =~ "placement"
    # Node counts are shown.
    assert html =~ "nodes"

    # Verify the LiveView is alive.
    assert render(view) =~ run_id
  end

  test "show page renders the graph svg element" do
    run_id = "live-graph-#{System.unique_integer([:positive])}"
    graph = build_graph(run_id, [agent_node("inspect"), agent_node("draft", inputs: %{"x" => {:node, "inspect", []}})])
    persist_graph(graph)

    {:ok, _view, html} = live(build_conn(), "/ir/" <> run_id)

    # The SVG graph component must be present.
    assert html =~ "<svg"
    assert html =~ "IR graph"
    # Node ids appear in the SVG.
    assert html =~ "inspect"
    assert html =~ "draft"
  end

  test "show page renders cancel button for a running run" do
    run_id = "live-cancel-btn-#{System.unique_integer([:positive])}"
    graph = build_graph(run_id, [agent_node("a")])
    persist_graph(graph)

    {:ok, _view, html} = live(build_conn(), "/ir/" <> run_id)
    assert html =~ "cancel run"
  end

  test "show page does not render cancel button for a succeeded run" do
    run_id = "live-no-cancel-#{System.unique_integer([:positive])}"

    graph =
      run_id
      |> build_graph([agent_node("a")])
      |> Map.put(:status, :succeeded)

    persist_graph(graph)

    {:ok, _view, html} = live(build_conn(), "/ir/" <> run_id)
    refute html =~ "cancel run"
  end

  test "show page renders retry_failed and rerun buttons for a failed run" do
    run_id = "live-failed-btns-#{System.unique_integer([:positive])}"

    node = %{agent_node("a") | state: :failed}

    graph =
      run_id
      |> build_graph([node])
      |> Map.put(:status, :failed)

    persist_graph(graph)

    {:ok, _view, html} = live(build_conn(), "/ir/" <> run_id)
    assert html =~ "retry failed"
    assert html =~ "rerun"
  end

  test "cancel button calls Runtime.cancel and run transitions to cancelled" do
    run_id = "live-cancel-action-#{System.unique_integer([:positive])}"

    # Use the default store dir so the Runtime, the LiveView, and the
    # assertion all read/write the same location. Clean up this run's file
    # after the test.
    default_ir_dir = Store.dir()
    File.mkdir_p!(default_ir_dir)
    on_exit(fn -> File.rm!(Path.join(default_ir_dir, run_id <> ".json")) end)

    # Build a graph with a blocking node so the run stays :running while we cancel.
    graph = build_graph(run_id, [agent_node("slow")])
    FakeEngine.program("slow", :block)

    # Start a real runtime using the default store so cancel has a live
    # process to reach and the store transition is visible.
    {:ok, _pid} = Runtime.start_link(graph, engine: FakeEngine)

    # Wait briefly for the runtime to persist the initial graph, then load
    # the LiveView and click cancel.
    assert eventually(fn ->
             match?({:ok, _}, Store.load(run_id))
           end),
           "run was not persisted by the runtime in time"

    {:ok, view, _html} = live(build_conn(), "/ir/" <> run_id)

    # Click cancel.
    render_click(view, "cancel")

    # The Runtime should now be cancelled. Poll the store until it reflects it.
    assert eventually(fn ->
             case Store.load(run_id) do
               {:ok, g} -> g.status == :cancelled
               _ -> false
             end
           end),
           "run #{run_id} did not become cancelled"
  end

  test "show page renders not-found message for an unknown run" do
    {:ok, _view, html} = live(build_conn(), "/ir/nonexistent-run-xyz")
    assert html =~ "run not found"
  end

  test "index paginates the runs table at 50 rows per page" do
    # Persist 51 runs into the default store the LiveView reads. They are
    # created now, so the latest-first sort floats all of them above any
    # leftover runs: page 1 is exactly the per-page cap and a 51st run spills
    # onto page 2.
    default_ir_dir = Store.dir()
    File.mkdir_p!(default_ir_dir)
    prefix = "live-page-#{System.unique_integer([:positive])}-"

    run_ids =
      for i <- 1..51 do
        run_id = prefix <> String.pad_leading(Integer.to_string(i), 3, "0")
        persist_graph(build_graph(run_id, [agent_node("a")]))
        run_id
      end

    on_exit(fn ->
      for run_id <- run_ids, do: File.rm(Path.join(default_ir_dir, run_id <> ".json"))
    end)

    {:ok, _view, html} = live(build_conn(), "/")
    # The pager renders and offers a second page once the cap is exceeded.
    assert html =~ ~s(class="pager")
    assert html =~ "page=2"
    # Page 1 shows exactly the per-page cap, never the full 51.
    assert count_run_rows(html) == 50

    {:ok, _view2, html2} = live(build_conn(), "/ir?page=2")
    # Page 2 carries the spillover and stays under the cap.
    rows2 = count_run_rows(html2)
    assert rows2 >= 1
    assert rows2 <= 50
  end

  # Each runs-table row links to its run at `/ir/<id>`; the pager links use
  # `?page=N` on the bare path, so counting the row-link prefix counts only
  # rendered run rows.
  defp count_run_rows(html) do
    (html |> String.split(~s(href="/ir/)) |> length()) - 1
  end

  test "placement_label renders fallback notation when declared != effective" do
    run_id = "live-placement-#{System.unique_integer([:positive])}"

    graph =
      run_id
      |> build_graph([agent_node("a")])
      |> Map.put(:placement, %{declared: :ixvm, effective: :host})

    persist_graph(graph)

    {:ok, _view, html} = live(build_conn(), "/ir/" <> run_id)
    assert html =~ "ixvm"
    assert html =~ "fallback"
    assert html =~ "host"
  end

  defp eventually(fun, attempts \\ 50) do
    cond do
      fun.() -> true
      attempts == 0 -> false
      true -> Process.sleep(20) && eventually(fun, attempts - 1)
    end
  end
end
