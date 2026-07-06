defmodule SymphonyElixirWeb.LinearWebhookController do
  @moduledoc """
  Receives Linear webhook events and starts IR runs for any `.sym`
  workflow whose `trigger.kind = :linear` label matches a label on the
  inbound issue.

  Replaces the old `Triggers.Linear` poller. Linear's 2500-req/hr quota
  is plenty when the poller is gone; webhooks add zero scheduled
  traffic.

  Setup, in Linear's webhook admin:

  - URL: `https://<symphony-host>/api/v1/triggers/linear`
  - Resource types: `Issue` (at minimum)
  - Copy the signing secret into `LINEAR_WEBHOOK_SECRET` on the
    symphony host

  Security: every request must carry a `Linear-Signature` header that is
  `hex(hmac_sha256(secret, raw_body))`, verified fail-closed by
  `SymphonyElixirWeb.WebhookAuth` over the exact bytes Linear signed.

  Dedupe: an issue with an active run (status `:pending` or
  `:running`) is skipped, matching the previous poller's contract.
  """

  use Phoenix.Controller, formats: [:json]

  alias SymphonyElixir.Runtime.{Ingress, Trigger}
  alias SymphonyElixirWeb.WebhookAuth

  require Logger

  @spec accept(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def accept(conn, params) do
    case WebhookAuth.verify(conn, :linear) do
      :ok ->
        handle_event(params)
        json(conn, %{ok: true})

      {:error, status, reason} ->
        Logger.warning("Linear webhook rejected: #{reason}")

        conn
        |> put_status(status)
        |> json(%{error: reason})
    end
  end

  defp handle_event(%{"type" => "Issue", "action" => action} = event)
       when action in ["create", "update"] do
    data = Map.get(event, "data", %{})
    labels = extract_labels(data)

    maybe_enqueue(data, labels)
  end

  defp handle_event(_event), do: :ok

  defp extract_labels(%{"labels" => labels}) when is_list(labels) do
    labels
    |> Enum.map(fn
      %{"name" => name} when is_binary(name) -> Trigger.normalize_label(name)
      _ -> nil
    end)
    |> Enum.reject(&is_nil/1)
  end

  defp extract_labels(%{"labelIds" => _ids}) do
    # Linear sends label ids only on some event shapes (e.g. older webhook
    # versions). We do not have the names locally; skip these events. The
    # next full update with a `labels` array will re-fire.
    []
  end

  defp extract_labels(_), do: []

  defp maybe_enqueue(%{"id" => issue_id} = data, labels) do
    if active_run_exists?(issue_id) do
      :ok
    else
      # The issue's labels ride on the event so the shared matcher can keep
      # the workflows whose declared label is present, fanning out to each.
      trigger = %{
        kind: :linear,
        labels: labels,
        issue_id: issue_id,
        identifier: Map.get(data, "identifier"),
        title: Map.get(data, "title"),
        url: Map.get(data, "url")
      }

      case Ingress.start_by_trigger(trigger) do
        {:ok, started} ->
          Logger.info("Started runs=#{Enum.map_join(started, ",", & &1.run_id)} for #{trigger.identifier} via webhook")

        {:error, reason} ->
          Logger.warning("Failed to start webhook run for #{trigger.identifier}: #{inspect(reason)}")
      end
    end
  end

  defp maybe_enqueue(_data, _labels), do: :ok

  defp active_run_exists?(linear_issue_id) do
    Ingress.seen_trigger?(fn
      {status, %{kind: :linear, issue_id: id}} -> id == linear_issue_id and status in [:pending, :running]
      {_status, _trigger} -> false
    end)
  end
end
