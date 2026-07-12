defmodule SymphonyElixirWeb.ApiController do
  @moduledoc """
  The manual-trigger enqueue producer onto the IR runtime.

      POST /api/v1/runs   start IR run(s) from a manual trigger;
                          body: {"workflow": "...", "input": {...}, "run_id": "..."}

  A caller naming a `workflow` starts exactly that `.sym`; a caller without
  one fires every `on manual` workflow through the shared trigger matcher.
  Input rides on the trigger context so a node can read it as `<input>`.

  A named start may include a caller-selected run id. The caller can record
  that id before sending the request and cancel the exact run even when the
  create response is lost. Every replay of an owned id returns 409, including
  an otherwise identical request. A run id without a workflow is rejected
  because one id cannot identify a fan-out start.
  """

  use Phoenix.Controller, formats: [:json]

  alias SymphonyElixir.Runtime.Ingress

  @spec enqueue_run(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def enqueue_run(conn, params) do
    input = Map.get(params, "input", %{})

    case {Map.get(params, "workflow") || Map.get(params, "dag"), Map.fetch(params, "run_id")} do
      {name, run_id} when is_binary(name) and name != "" ->
        name
        |> Ingress.start_by_name(%{kind: :manual, input: input}, run_opts(run_id))
        |> respond_started(conn)

      {_name, {:ok, _run_id}} ->
        conn
        |> put_status(:unprocessable_entity)
        |> json(%{error: "run_id requires a named workflow"})

      {_name, :error} ->
        %{kind: :manual, input: input}
        |> Ingress.start_by_trigger([])
        |> respond_started(conn)
    end
  end

  defp respond_started({:ok, %{run_id: run_id}}, conn), do: conn |> put_status(:created) |> json(%{run_ids: [run_id]})

  defp respond_started({:ok, started}, conn) when is_list(started), do: conn |> put_status(:created) |> json(%{run_ids: Enum.map(started, & &1.run_id)})

  defp respond_started({:error, {:workflow_not_found, _}} = reason, conn), do: conn |> put_status(:not_found) |> json(%{error: inspect(reason)})

  defp respond_started({:error, {:run_id_conflict, run_id}}, conn), do: conn |> put_status(:conflict) |> json(%{error: "run id already exists: #{run_id}"})

  defp respond_started({:error, reason}, conn), do: conn |> put_status(:unprocessable_entity) |> json(%{error: inspect(reason)})

  defp run_opts(:error), do: []
  defp run_opts({:ok, run_id}), do: [run_id: run_id]
end
