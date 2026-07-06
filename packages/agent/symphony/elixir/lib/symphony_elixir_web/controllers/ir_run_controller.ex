defmodule SymphonyElixirWeb.IRRunController do
  @moduledoc """
  Read-only JSON API over IR runs (the `RunGraph` model), and the operator
  control endpoints.

      GET  /api/v1/ir/schema               the runtime's enum vocabulary
      GET  /api/v1/ir/runs                 list IR runs (summaries)
      POST /api/v1/ir/runs                 start a run from a workflow name
      GET  /api/v1/ir/runs/:run_id         one IR run (full detail)
      POST /api/v1/ir/runs/:run_id/cancel        operator: cancel
      POST /api/v1/ir/runs/:run_id/rerun         operator: re-run all
      POST /api/v1/ir/runs/:run_id/clear-failed  operator: clear failed nodes
      POST /api/v1/ir/runs/:run_id/nodes/:node_id/retry  operator: retry one node

  This is parallel to the legacy `/api/v1/runs` surface (the old `Run`
  model) and renders the canonical IR facts through `IR.View`, keeping the
  protocol emitter out of the runtime. Reads come from `IR.Store` so a
  finished or restarted run is visible; operator actions go to the live
  `Runtime` process, returning 409 when the run has no live process to act
  on (a succeeded or cancelled run that already stopped).
  """

  use Phoenix.Controller, formats: [:json]

  alias SymphonyElixir.DSL.Schema
  alias SymphonyElixir.IR.Store
  alias SymphonyElixir.IR.View
  alias SymphonyElixir.Runtime

  # The runtime's single source of truth for the form's option lists:
  # engines, efforts, permissions, locations, node kinds/states, effect
  # kinds, and trigger kinds. A consumer drives its selects from this so a
  # new enum value at its owner reaches the UI without a form edit.
  @spec schema(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def schema(conn, _params) do
    json(conn, Schema.to_map())
  end

  @spec index(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def index(conn, _params) do
    summaries = Store.load_all() |> Enum.sort_by(& &1.run_id) |> Enum.map(&View.summary/1)
    json(conn, %{runs: summaries})
  end

  # Start a run from a workflow name. This is the manual/operator door onto
  # the IR runtime: resolve the workflow through the catalog, materialize
  # it, and start it under Runtime.Supervisor. Trigger context is optional;
  # an operator-started run carries `%{kind: :manual}` plus any input the
  # caller passed.
  @spec create(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def create(conn, %{"workflow" => name}) when is_binary(name) do
    case Runtime.Ingress.start_by_name(name, trigger_context(conn.params), []) do
      {:ok, %{run_id: run_id}} ->
        conn |> put_status(:created) |> json(%{run_id: run_id})

      {:error, {:workflow_not_found, _}} = reason ->
        conn |> put_status(:not_found) |> json(%{error: inspect(reason)})

      {:error, reason} ->
        conn |> put_status(:unprocessable_entity) |> json(%{error: inspect(reason)})
    end
  end

  @spec create(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def create(conn, _params) do
    conn |> put_status(:unprocessable_entity) |> json(%{error: "missing required field: workflow"})
  end

  # Build the trigger context from request params. A manual run always
  # carries `kind: :manual`; any caller-supplied `input` map rides along so
  # a node can read it as `<input>`. Absent or non-map input defaults to an
  # empty map so the graph trigger shape is stable.
  defp trigger_context(params) do
    input =
      case params["input"] do
        %{} = map -> map
        _ -> %{}
      end

    %{kind: :manual, input: input}
  end

  @spec show(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def show(conn, %{"run_id" => run_id}) do
    case Store.load(run_id) do
      {:ok, graph} -> json(conn, View.detail(graph))
      {:error, :not_found} -> not_found(conn)
      {:error, reason} -> conn |> put_status(:unprocessable_entity) |> json(%{error: inspect(reason)})
    end
  end

  @spec cancel(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def cancel(conn, %{"run_id" => run_id}), do: operate(conn, run_id, &Runtime.cancel(&1, actor(conn)))

  @spec rerun(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def rerun(conn, %{"run_id" => run_id}), do: operate(conn, run_id, &Runtime.rerun(&1, actor(conn)))

  @spec clear_failed(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def clear_failed(conn, %{"run_id" => run_id}), do: operate(conn, run_id, &Runtime.clear_failed(&1, actor(conn)))

  @spec retry_node(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def retry_node(conn, %{"run_id" => run_id, "node_id" => node_id}), do: operate(conn, run_id, &Runtime.retry_node(&1, node_id, actor(conn)))

  # Apply an operator action to the live run, then return its current
  # persisted detail. A run with no live process (already stopped) returns
  # 409 with a clear reason rather than a 500 from the GenServer call.
  defp operate(conn, run_id, action) do
    action.(run_id)

    case Store.load(run_id) do
      {:ok, graph} -> json(conn, View.detail(graph))
      {:error, :not_found} -> not_found(conn)
      {:error, reason} -> conn |> put_status(:unprocessable_entity) |> json(%{error: inspect(reason)})
    end
  catch
    :exit, {:noproc, _} -> run_not_live(conn, run_id)
    :exit, {{:noproc, _}, _} -> run_not_live(conn, run_id)
  end

  defp actor(conn) do
    case get_req_header(conn, "x-operator") do
      [value | _] when value != "" -> value
      _ -> :operator
    end
  end

  defp not_found(conn), do: conn |> put_status(:not_found) |> json(%{error: "run not found"})

  defp run_not_live(conn, run_id) do
    conn
    |> put_status(:conflict)
    |> json(%{error: "run #{run_id} has no live process; it has already finished or is not running"})
  end
end
