defmodule SymphonyElixirWeb.SlackEventsController do
  @moduledoc "Receives Slack Events API callbacks and starts app-mention IR runs."

  use Phoenix.Controller, formats: [:json]

  require Logger

  alias SymphonyElixir.{Config, Slack, WorkflowCatalog}
  alias SymphonyElixir.Runtime.Ingress

  @spec accept(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def accept(conn, %{"type" => "url_verification", "challenge" => challenge}) do
    with :ok <- verify_signature(conn) do
      json(conn, %{challenge: challenge})
    else
      {:error, status, reason} ->
        conn |> put_status(status) |> json(%{error: reason})
    end
  end

  def accept(conn, %{"event" => %{"type" => "app_mention"} = event}) do
    with :ok <- verify_signature(conn) do
      json(conn, handle_app_mention(event))
    else
      {:error, status, reason} ->
        Logger.warning("Slack event rejected: #{reason}")
        conn |> put_status(status) |> json(%{error: reason})
    end
  end

  def accept(conn, _params) do
    with :ok <- verify_signature(conn) do
      json(conn, %{ok: true, ignored: true})
    else
      {:error, status, reason} ->
        conn |> put_status(status) |> json(%{error: reason})
    end
  end

  defp handle_app_mention(event) do
    channel = Map.get(event, "channel")
    ts = Map.get(event, "ts")
    thread_ts = Map.get(event, "thread_ts") || ts

    cond do
      not is_binary(channel) or not is_binary(ts) ->
        %{ok: true, results: [format_result({:ignored, "missing channel or ts"})]}

      active_run_exists?(channel, ts) ->
        %{ok: true, results: [format_result({:deduped, ts})]}

      true ->
        # Stamp both the raw channel id the event carries and any declared
        # channel name resolved to it, so the shared matcher accepts a
        # workflow that declared either spelling.
        trigger = %{
          kind: :slack_app_mention,
          channel: resolved_channel_name(channel) || channel,
          channel_id: channel,
          message_ts: ts,
          thread_ts: thread_ts,
          user: Map.get(event, "user"),
          text: Map.get(event, "text", "")
        }

        start_mention(trigger)
    end
  end

  defp start_mention(trigger) do
    case Ingress.start_by_trigger(trigger) do
      {:ok, started} ->
        %{ok: true, enqueued: length(started), results: Enum.map(started, &format_result({:enqueued, &1.run_id}))}

      {:error, reason} ->
        %{ok: true, results: [format_result({:error, inspect(reason)})]}
    end
  end

  # Resolve the channel id back to the `#name` a workflow's `on` clause
  # might declare, so a name-based trigger and the event's id compare
  # equal. The candidate names come from the loaded `:slack_app_mention`
  # workflows, so this only resolves names symphony actually watches.
  defp resolved_channel_name(channel_id) do
    WorkflowCatalog.for_trigger_kind(:slack_app_mention)
    |> Enum.map(& &1.trigger.channel)
    |> Enum.uniq()
    |> Enum.find(fn declared -> channel_matches?(declared, channel_id) end)
  end

  defp channel_matches?("#" <> channel_name, channel_id) do
    case Slack.Client.resolve_channel_id(channel_name) do
      {:ok, ^channel_id} -> true
      _ -> false
    end
  end

  defp channel_matches?(configured, channel_id), do: configured == channel_id

  defp active_run_exists?(channel, ts) do
    Ingress.seen_trigger?(fn
      {status, %{kind: :slack_app_mention, channel_id: cid, message_ts: mts}} ->
        status in [:pending, :running] and cid == channel and mts == ts

      {_status, _trigger} ->
        false
    end)
  end

  defp verify_signature(conn) do
    secret = Config.get().slack_signing_secret

    cond do
      is_nil(secret) ->
        {:error, :unauthorized, "slack signing secret not configured"}

      is_nil(conn.assigns[:raw_body]) ->
        {:error, :bad_request, "missing raw body"}

      true ->
        timestamp = conn |> Plug.Conn.get_req_header("x-slack-request-timestamp") |> List.first()
        provided = conn |> Plug.Conn.get_req_header("x-slack-signature") |> List.first()
        expected = expected_signature(secret, timestamp, conn.assigns.raw_body)

        cond do
          is_nil(timestamp) or is_nil(provided) ->
            {:error, :unauthorized, "missing Slack signature headers"}

          byte_size(provided) != byte_size(expected) ->
            {:error, :unauthorized, "signature mismatch"}

          not Plug.Crypto.secure_compare(provided, expected) ->
            {:error, :unauthorized, "signature mismatch"}

          true ->
            :ok
        end
    end
  end

  defp expected_signature(secret, timestamp, body) do
    base = "v0:" <> to_string(timestamp) <> ":" <> body
    digest = :crypto.mac(:hmac, :sha256, secret, base) |> Base.encode16(case: :lower)
    "v0=" <> digest
  end

  defp format_result({:enqueued, run_id}), do: %{status: "enqueued", run_id: run_id}
  defp format_result({:deduped, ts}), do: %{status: "deduped", message_ts: ts}
  defp format_result({:ignored, reason}), do: %{status: "ignored", reason: reason}
  defp format_result({:error, reason}), do: %{status: "error", reason: reason}
end
