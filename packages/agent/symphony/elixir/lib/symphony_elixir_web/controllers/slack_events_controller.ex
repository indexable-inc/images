defmodule SymphonyElixirWeb.SlackEventsController do
  @moduledoc """
  Receives Slack Events API callbacks and starts app-mention IR runs.
  Requests are authenticated by `SymphonyElixirWeb.WebhookAuth` against
  `SLACK_SIGNING_SECRET`.
  """

  use Phoenix.Controller, formats: [:json]

  alias SymphonyElixir.Runtime.Ingress
  alias SymphonyElixir.Slack
  alias SymphonyElixir.WorkflowCatalog
  alias SymphonyElixirWeb.TriggerResponse
  alias SymphonyElixirWeb.WebhookAuth

  require Logger

  @spec accept(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def accept(conn, %{"type" => "url_verification", "challenge" => challenge}) do
    case WebhookAuth.verify(conn, :slack) do
      :ok ->
        json(conn, %{challenge: challenge})

      {:error, status, reason} ->
        conn |> put_status(status) |> json(%{error: reason})
    end
  end

  @spec accept(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def accept(conn, %{"event" => %{"type" => "app_mention"} = event}) do
    case WebhookAuth.verify(conn, :slack) do
      :ok ->
        json(conn, handle_app_mention(event))

      {:error, status, reason} ->
        Logger.warning("Slack event rejected: #{reason}")
        conn |> put_status(status) |> json(%{error: reason})
    end
  end

  @spec accept(Plug.Conn.t(), map()) :: Plug.Conn.t()
  def accept(conn, _params) do
    case WebhookAuth.verify(conn, :slack) do
      :ok ->
        json(conn, %{ok: true, ignored: true})

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
        %{ok: true, results: [TriggerResponse.format_result({:ignored, "missing channel or ts"})]}

      active_run_exists?(channel, ts) ->
        %{ok: true, results: [TriggerResponse.format_result({:deduped, %{message_ts: ts}})]}

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

        TriggerResponse.start_by_trigger(trigger, "#{channel}@#{ts} via slack app mention")
    end
  end

  # Resolve the channel id back to the `#name` a workflow's `on` clause
  # might declare, so a name-based trigger and the event's id compare
  # equal. The candidate names come from the loaded `:slack_app_mention`
  # workflows, so this only resolves names symphony actually watches.
  defp resolved_channel_name(channel_id) do
    :slack_app_mention
    |> WorkflowCatalog.for_trigger_kind()
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
end
