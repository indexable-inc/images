defmodule SymphonyElixirWeb.TriggerResponse do
  @moduledoc """
  The webhook controllers' shared response vocabulary: one formatter for
  the per-run `results` entries and one start-log-respond step over
  `Runtime.Ingress`, so the github and slack controllers cannot drift
  apart on response field names or start/failure logging.
  """

  alias SymphonyElixir.Runtime.Ingress

  require Logger

  @typedoc "One per-run entry in a trigger response's `results` list."
  @type result ::
          {:enqueued, String.t()}
          | {:deduped, map()}
          | {:ignored, String.t()}
          | {:error, String.t()}

  @doc """
  Start every workflow matching `trigger` and shape the JSON body the
  controller answers with. `subject` names the event in the log line
  (e.g. `"acme/widgets#7 via github label"`). A failed start still
  answers `ok: true`: the webhook was authenticated and handled, so the
  failure is symphony's to log, not the provider's to retry.
  """
  @spec start_by_trigger(map(), String.t()) :: map()
  def start_by_trigger(trigger, subject) do
    case Ingress.start_by_trigger(trigger) do
      {:ok, started} ->
        Logger.info("Started runs=#{Enum.map_join(started, ",", & &1.run_id)} for #{subject}")
        %{ok: true, enqueued: length(started), results: Enum.map(started, &format_result({:enqueued, &1.run_id}))}

      {:error, reason} ->
        Logger.warning("Failed to start run for #{subject}: #{inspect(reason)}")
        %{ok: true, results: [format_result({:error, inspect(reason)})]}
    end
  end

  @doc """
  One `results` entry. `:deduped` carries a map because each provider
  dedups on its own key (`pr_number`, `message_ts`); the map is merged
  into the entry beside the status.
  """
  @spec format_result(result()) :: map()
  def format_result({:enqueued, run_id}), do: %{status: "enqueued", run_id: run_id}
  def format_result({:deduped, extra}) when is_map(extra), do: Map.put(extra, :status, "deduped")
  def format_result({:ignored, reason}), do: %{status: "ignored", reason: reason}
  def format_result({:error, reason}), do: %{status: "error", reason: reason}
end
