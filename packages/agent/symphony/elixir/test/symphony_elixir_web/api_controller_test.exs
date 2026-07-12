defmodule SymphonyElixirWeb.ApiControllerTest do
  use ExUnit.Case, async: false

  import Plug.Conn
  import Plug.Test

  alias SymphonyElixir.DSL.Parser
  alias SymphonyElixir.IR.Store
  alias SymphonyElixir.Runtime

  @opts SymphonyElixirWeb.Endpoint.init([])

  setup do
    if !Process.whereis(SymphonyElixir.Runtime.Registry) do
      start_supervised!({Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry})
    end

    if !Process.whereis(SymphonyElixir.TaskSupervisor) do
      start_supervised!({Task.Supervisor, name: SymphonyElixir.TaskSupervisor})
    end

    if !Process.whereis(SymphonyElixir.Runtime.Supervisor) do
      start_supervised!(SymphonyElixir.Runtime.Supervisor)
    end

    ensure_workflow_catalog_table()

    store_dir = Store.dir()
    File.rm_rf!(store_dir)
    File.mkdir_p!(store_dir)

    script = Path.join(SymphonyElixir.Config.get().pack_dir, "scripts/api-controller-slow.sh")
    File.mkdir_p!(Path.dirname(script))
    File.write!(script, "#!/bin/sh\nsleep 30\n")
    File.chmod!(script, 0o755)

    on_exit(fn ->
      File.rm_rf!(store_dir)
      File.rm!(script)
    end)

    :ok
  end

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
    entry
  end

  defp post(path, body) do
    :post
    |> conn(path, Jason.encode!(body))
    |> put_req_header("content-type", "application/json")
    |> SymphonyElixirWeb.Endpoint.call(@opts)
  end

  defp slow_workflow(name, node_id \\ "wait") do
    ~s|workflow "#{name}" on manual { #{node_id} <- exec "./scripts/api-controller-slow.sh" timeout 60 }|
  end

  defp cancel_on_exit(run_id) do
    on_exit(fn -> cancel_if_registered(Process.whereis(SymphonyElixir.Runtime.Registry), run_id) end)
  end

  defp cancel_if_registered(nil, _run_id), do: :ok

  defp cancel_if_registered(_registry, run_id) do
    case Registry.lookup(SymphonyElixir.Runtime.Registry, run_id) do
      [{pid, _value}] -> Runtime.cancel(pid, :test_cleanup)
      [] -> :ok
    end
  end

  test "POST /api/v1/runs accepts a caller-supplied run id" do
    put_workflow("slow", slow_workflow("slow"))
    run_id = "caller-selected-create"
    cancel_on_exit(run_id)

    conn = post("/api/v1/runs", %{"workflow" => "slow", "run_id" => run_id, "input" => %{"proof" => "create"}})

    assert conn.status == 201
    assert Jason.decode!(conn.resp_body) == %{"run_ids" => [run_id]}
    assert {:ok, graph} = Store.load(run_id)
    assert graph.trigger == %{kind: :manual, input: %{"proof" => "create"}}
  end

  test "an identical caller-supplied run id replay is an explicit conflict" do
    entry = put_workflow("slow", slow_workflow("slow"))
    run_id = "caller-selected-replay"
    cancel_on_exit(run_id)
    request = %{"workflow" => "slow", "run_id" => run_id, "input" => %{"proof" => "same"}}

    assert post("/api/v1/runs", request).status == 201
    replay = post("/api/v1/runs", request)

    assert replay.status == 409
    assert Jason.decode!(replay.resp_body) == %{"error" => "run id already exists: #{run_id}"}
    assert {:ok, graph} = Store.load(run_id)
    assert graph.source_hash == entry.hash
    assert graph.trigger == %{kind: :manual, input: %{"proof" => "same"}}
  end

  test "a duplicate run id cannot adopt a different workflow or input" do
    first = put_workflow("first", slow_workflow("first"))
    put_workflow("second", slow_workflow("second", "other"))
    run_id = "caller-selected-mismatch"
    cancel_on_exit(run_id)

    assert post("/api/v1/runs", %{"workflow" => "first", "run_id" => run_id, "input" => %{"owner" => "first"}}).status == 201

    duplicate = post("/api/v1/runs", %{"workflow" => "second", "run_id" => run_id, "input" => %{"owner" => "second"}})

    assert duplicate.status == 409
    assert Jason.decode!(duplicate.resp_body) == %{"error" => "run id already exists: #{run_id}"}
    assert {:ok, graph} = Store.load(run_id)
    assert graph.source_hash == first.hash
    assert graph.trigger == %{kind: :manual, input: %{"owner" => "first"}}
  end

  test "a preselected run id cancels a run after its create response is lost" do
    put_workflow("slow", slow_workflow("slow"))
    run_id = "caller-selected-lost-response"
    cancel_on_exit(run_id)

    _dropped_response = post("/api/v1/runs", %{"workflow" => "slow", "run_id" => run_id})
    cancelled = post("/api/v1/ir/runs/#{run_id}/cancel", %{})

    assert cancelled.status == 200
    assert Jason.decode!(cancelled.resp_body)["run_id"] == run_id
    assert {:ok, %{status: :cancelled}} = Store.load(run_id)
  end

  test "caller-supplied run ids are validated at Ingress" do
    put_workflow("slow", slow_workflow("slow"))

    for invalid <- ["", "../escape", "UPPER", "trailing-", String.duplicate("a", 129), 42] do
      conn = post("/api/v1/runs", %{"workflow" => "slow", "run_id" => invalid})

      assert conn.status == 422
      assert Jason.decode!(conn.resp_body)["error"] =~ "invalid_run_id"
    end

    assert Store.load_all() == []
  end

  test "a caller-supplied run id requires one named workflow" do
    put_workflow("slow", slow_workflow("slow"))

    conn = post("/api/v1/runs", %{"run_id" => "fan-out-cannot-own-one-id"})

    assert conn.status == 422
    assert Jason.decode!(conn.resp_body) == %{"error" => "run_id requires a named workflow"}
    assert Store.load_all() == []
  end
end
