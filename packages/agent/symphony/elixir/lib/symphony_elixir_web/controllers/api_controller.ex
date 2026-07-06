defmodule SymphonyElixirWeb.ApiController do
  @moduledoc """
  The manual-trigger enqueue producer onto the IR runtime.

      POST /api/v1/runs   start IR run(s) from a manual trigger;
                          body: {"workflow": "...", "input": {...}}

  A caller naming a `workflow` starts exactly that `.sym`; a caller without
  one fires every `on manual` workflow through the shared trigger matcher.
  Input rides on the trigger context so a node can read it as `<input>`.
  """

  use Phoenix.Controller, formats: [:json]

  alias SymphonyElixir.Runtime.Ingress

  @spec enqueue_run(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def enqueue_run(conn, params) do
    input = Map.get(params, "input", %{})

    case Map.get(params, "workflow") || Map.get(params, "dag") do
      name when is_binary(name) and name != "" ->
        name
        |> Ingress.start_by_name(%{kind: :manual, input: input}, [])
        |> respond_started(conn)

      _ ->
        %{kind: :manual, input: input}
        |> Ingress.start_by_trigger([])
        |> respond_started(conn)
    end
  end

  defp respond_started({:ok, %{run_id: run_id}}, conn), do: conn |> put_status(:created) |> json(%{run_ids: [run_id]})

  defp respond_started({:ok, started}, conn) when is_list(started), do: conn |> put_status(:created) |> json(%{run_ids: Enum.map(started, & &1.run_id)})

  defp respond_started({:error, {:workflow_not_found, _}} = reason, conn), do: conn |> put_status(:not_found) |> json(%{error: inspect(reason)})

  defp respond_started({:error, reason}, conn), do: conn |> put_status(:unprocessable_entity) |> json(%{error: inspect(reason)})
end
