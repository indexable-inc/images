defmodule SymphonyElixirWeb.IRRunControllerTest do
  use ExUnit.Case, async: false
  import Plug.Test
  import Plug.Conn

  alias SymphonyElixir.DSL.{Parser, Schema}
  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.{Node, RunGraph, Store}

  @opts SymphonyElixirWeb.Endpoint.init([])

  # The controller reads the IR store at its default dir
  # (Config.get().runs_dir/ir). Clean it between tests so listings are
  # deterministic.
  setup do
    # The Runtime.Registry must exist for operator routes to resolve a run
    # name; a run that is not registered then yields the :noproc the
    # controller translates to 409. Start it if the Application is not up.
    unless Process.whereis(SymphonyElixir.Runtime.Registry) do
      start_supervised!({Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry})
    end

    # The create route resolves a workflow through WorkflowCatalog's ETS
    # table and starts it under Runtime.Supervisor. Bring up both when the
    # Application is not running (auto_start: false in test).
    ensure_workflow_catalog_table()

    unless Process.whereis(SymphonyElixir.TaskSupervisor) do
      start_supervised!({Task.Supervisor, name: SymphonyElixir.TaskSupervisor})
    end

    unless Process.whereis(SymphonyElixir.Runtime.Supervisor) do
      start_supervised!(SymphonyElixir.Runtime.Supervisor)
    end

    dir = Path.join(SymphonyElixir.Config.get().runs_dir, "ir")
    File.rm_rf(dir)
    File.mkdir_p!(dir)
    :ok
  end

  # The catalog table is created by the WorkflowCatalog GenServer at boot,
  # which test_helper does not start. Create it here so put_workflow/1 and
  # the create route can read it, and reset its rows each test.
  defp ensure_workflow_catalog_table do
    table = :symphony_workflows

    if :ets.whereis(table) == :undefined do
      :ets.new(table, [:named_table, :public, read_concurrency: true])
    else
      :ets.delete_all_objects(table)
    end
  end

  defp put_workflow(name, source) do
    {:ok, ast} = Parser.parse(source)
    entry = %{name: ast.name || name, ast: ast, trigger: ast.trigger, source: source, hash: :crypto.hash(:sha256, source)}
    :ets.insert(:symphony_workflows, {name, entry})
  end

  defp persist_run(run_id, status) do
    node = %{Node.new(id: "a", ast_origin: {:exec, "a"}, kind: :exec, inputs: %{}) | state: :succeeded, output: %{"v" => 1}}
    graph = RunGraph.new(run_id, "hash", nil) |> RunGraph.put_nodes([node]) |> Map.put(:status, status)
    :ok = Store.persist(graph)
  end

  defp get(path) do
    :get |> conn(path) |> SymphonyElixirWeb.Endpoint.call(@opts)
  end

  defp post(path) do
    :post |> conn(path) |> put_req_header("content-type", "application/json") |> SymphonyElixirWeb.Endpoint.call(@opts)
  end

  defp post(path, body) do
    :post
    |> conn(path, Jason.encode!(body))
    |> put_req_header("content-type", "application/json")
    |> SymphonyElixirWeb.Endpoint.call(@opts)
  end

  test "GET /api/v1/ir/schema returns the runtime enum vocabulary" do
    conn = get("/api/v1/ir/schema")
    assert conn.status == 200
    body = Jason.decode!(conn.resp_body)

    # The endpoint serves the runtime's accessors verbatim (atoms render as
    # strings), so the form's option lists match what a turn accepts. Assert
    # against the accessor, not a second hardcoded list, so the test is not
    # itself a place the vocabulary can drift.
    assert body["engines"] == strings(Envelope.engines())
    assert body["permissions"] == strings(Envelope.permission_levels())
    assert "agent" in body["node_kinds"]
    assert "manual" in body["trigger_kinds"]
  end

  test "schema enums, struct-accepted values, and the API payload do not drift" do
    # ENG-1825's "the UI cannot drift from the runtime" pillar. Three sources
    # must name the same vocabulary or the form offers options a turn rejects:
    #   1. the accessors the schema map reads (what the form renders),
    #   2. the values Envelope.from_map/1 actually accepts (what a turn takes),
    #   3. the JSON the /schema endpoint serves (what ships over the wire).
    # A value added to one but not another turns this red. The generated
    # @type unions keep Dialyzer in agreement with leg 2 at compile time;
    # this test covers the runtime legs Dialyzer cannot see.
    schema = Schema.to_map()
    api = "/api/v1/ir/schema" |> get() |> Map.fetch!(:resp_body) |> Jason.decode!()

    # engines: each accepted with an engine-agreeing model; an off-list value rejected.
    assert schema.engines == Envelope.engines()
    assert api["engines"] == strings(Envelope.engines())

    for engine <- Envelope.engines() do
      assert {:ok, %{engine: ^engine}} =
               Envelope.from_map(%{"engine" => engine, "model" => model_for(engine)})
    end

    assert {:error, {:invalid_engine, _}} =
             Envelope.from_map(%{"engine" => :nonsense, "model" => "m"})

    # efforts
    assert schema.efforts == Envelope.efforts()
    assert api["efforts"] == strings(Envelope.efforts())

    for effort <- Envelope.efforts() do
      assert {:ok, %{effort: ^effort}} =
               Envelope.from_map(%{"engine" => :codex, "model" => "m", "effort" => effort})
    end

    assert {:error, {:invalid_effort, _}} =
             Envelope.from_map(%{"engine" => :codex, "model" => "m", "effort" => :nope})

    # permissions
    assert schema.permissions == Envelope.permission_levels()
    assert api["permissions"] == strings(Envelope.permission_levels())

    for perm <- Envelope.permission_levels() do
      assert {:ok, %{permissions: ^perm}} =
               Envelope.from_map(%{"engine" => :codex, "model" => "m", "permissions" => perm})
    end

    assert {:error, {:invalid_permissions, _}} =
             Envelope.from_map(%{"engine" => :codex, "model" => "m", "permissions" => :nope})

    # locations: the bare placement tags the form offers (payload-carriers
    # supply their payload separately, so only the tag list is the shared axis).
    assert schema.locations == Envelope.locations()
    assert api["locations"] == strings(Envelope.locations())
  end

  defp strings(atoms), do: Enum.map(atoms, &Atom.to_string/1)

  # check_engine_model_agree rejects a Claude model under :codex and a
  # non-Claude model under :claude, so each engine needs an agreeing model.
  defp model_for(:codex), do: "gpt-5.3-codex"
  defp model_for(:claude), do: "claude-opus-4-8"
  defp model_for(:pi), do: "claude"

  test "GET /api/v1/ir/runs lists persisted run summaries" do
    persist_run("run_a", :succeeded)
    persist_run("run_b", :failed)

    conn = get("/api/v1/ir/runs")
    assert conn.status == 200
    body = Jason.decode!(conn.resp_body)
    ids = Enum.map(body["runs"], & &1["run_id"])
    assert ids == ["run_a", "run_b"]
    assert Enum.find(body["runs"], &(&1["run_id"] == "run_b"))["status"] == "failed"
  end

  test "GET /api/v1/ir/runs/:id returns the full detail" do
    persist_run("run_detail", :succeeded)

    conn = get("/api/v1/ir/runs/run_detail")
    assert conn.status == 200
    body = Jason.decode!(conn.resp_body)
    assert body["run_id"] == "run_detail"
    assert [node] = body["nodes"]
    assert node["id"] == "a"
    assert node["output"] == %{"v" => 1}
  end

  test "GET an unknown run returns 404" do
    conn = get("/api/v1/ir/runs/nope")
    assert conn.status == 404
    assert Jason.decode!(conn.resp_body) == %{"error" => "run not found"}
  end

  test "POST /api/v1/ir/runs starts a run from a workflow name" do
    put_workflow("demo", ~s|workflow "demo" on manual { a <- agent { engine: codex, model: "m", prompt: inline "go" } }|)

    conn = post("/api/v1/ir/runs", %{"workflow" => "demo"})
    assert conn.status == 201
    body = Jason.decode!(conn.resp_body)
    assert is_binary(body["run_id"])
    assert String.starts_with?(body["run_id"], "demo-")

    # The run is materialized and persisted, so it is visible on the index.
    run_id = body["run_id"]

    assert eventually(fn ->
             match?({:ok, _}, Store.load(run_id))
           end)
  end

  test "POST /api/v1/ir/runs for an unknown workflow returns 404" do
    conn = post("/api/v1/ir/runs", %{"workflow" => "nope"})
    assert conn.status == 404
    assert Jason.decode!(conn.resp_body)["error"] =~ "workflow_not_found"
  end

  test "POST /api/v1/ir/runs without a workflow field returns 422" do
    conn = post("/api/v1/ir/runs", %{})
    assert conn.status == 422
    assert Jason.decode!(conn.resp_body)["error"] =~ "workflow"
  end

  defp eventually(fun, attempts \\ 50) do
    cond do
      fun.() -> true
      attempts == 0 -> false
      true -> Process.sleep(20) && eventually(fun, attempts - 1)
    end
  end

  test "operator action on a run with no live process returns 409" do
    # A persisted run with no live Runtime GenServer: cancel cannot reach a
    # process, so the controller returns 409 rather than a 500.
    persist_run("run_dead", :failed)

    conn = post("/api/v1/ir/runs/run_dead/cancel")
    assert conn.status == 409
    assert Jason.decode!(conn.resp_body)["error"] =~ "no live process"
  end
end
