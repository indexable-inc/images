defmodule SymphonyElixir.Slack.Client do
  @moduledoc """
  Thin Slack Web API client used by the huddle-completion trigger.

  Two responsibilities:

  - Resolve a channel name to an id via `conversations.list`.
  - Page `conversations.history` for that channel id over a recent window.

  Auth comes from `Config.get().slack_bot_token`. When the bot token is
  absent the client returns `{:error, :missing_slack_token}` and the
  trigger stays idle.

  Skills that need a Slack user token (e.g. `focus_route` for fetching
  `huddle_transcript` files via `files.sharedPublicURL`) read
  `SLACK_USER_OAUTH_TOKEN` directly from the inherited subprocess env;
  Symphony does not proxy it through Config.

  This module knows nothing about huddles or DAG triggering. Filtering
  for completed huddles lives in `SymphonyElixir.Triggers.Slack`.
  """

  alias SymphonyElixir.Config

  require Logger

  @page_size 100

  @spec resolve_channel_id(String.t()) :: {:ok, String.t()} | {:error, term()}
  def resolve_channel_id(channel_name) when is_binary(channel_name) do
    trimmed = String.trim_leading(channel_name, "#")

    with {:ok, token} <- bot_token() do
      walk_channel_list(token, trimmed, nil)
    end
  end

  @spec conversations_history(String.t(), keyword()) :: {:ok, [map()]} | {:error, term()}
  def conversations_history(channel_id, opts \\ []) when is_binary(channel_id) do
    with {:ok, token} <- bot_token() do
      params =
        opts
        |> Keyword.take([:oldest, :latest, :limit])
        |> Enum.into(%{"channel" => channel_id, "limit" => @page_size})
        |> Map.new(fn {k, v} -> {to_string(k), v} end)

      case slack_get(token, "conversations.history", params) do
        {:ok, %{"messages" => messages}} -> {:ok, messages}
        {:error, _} = err -> err
      end
    end
  end

  @spec conversations_replies(String.t(), String.t(), keyword()) :: {:ok, [map()]} | {:error, term()}
  def conversations_replies(channel_id, thread_ts, opts \\ []) when is_binary(channel_id) and is_binary(thread_ts) do
    with {:ok, token} <- bot_token() do
      params =
        opts
        |> Keyword.take([:limit])
        |> Enum.into(%{"channel" => channel_id, "ts" => thread_ts, "limit" => @page_size})
        |> Map.new(fn {k, v} -> {to_string(k), v} end)

      case slack_get(token, "conversations.replies", params) do
        {:ok, %{"messages" => messages}} -> {:ok, messages}
        {:error, _} = err -> err
      end
    end
  end

  @spec post_message(String.t(), map()) :: {:ok, map()} | {:error, term()}
  def post_message(channel_id, payload) when is_binary(channel_id) and is_map(payload) do
    with {:ok, token} <- bot_token() do
      payload = Map.put(payload, "channel", channel_id)
      slack_post(token, "chat.postMessage", payload)
    end
  end

  defp walk_channel_list(token, name, cursor) do
    params = maybe_put_cursor(%{"limit" => 1000, "exclude_archived" => "true", "types" => "public_channel,private_channel"}, cursor)

    case slack_get(token, "conversations.list", params) do
      {:ok, %{"channels" => channels} = body} ->
        case Enum.find(channels, fn ch -> ch["name"] == name end) do
          %{"id" => id} ->
            {:ok, id}

          nil ->
            case get_in(body, ["response_metadata", "next_cursor"]) do
              c when is_binary(c) and c != "" -> walk_channel_list(token, name, c)
              _ -> {:error, {:channel_not_found, name}}
            end
        end

      {:error, _} = err ->
        err
    end
  end

  defp maybe_put_cursor(params, nil), do: params
  defp maybe_put_cursor(params, ""), do: params
  defp maybe_put_cursor(params, cursor), do: Map.put(params, "cursor", cursor)

  defp slack_get(token, method, params) do
    config = Config.get()
    url = config.slack_endpoint <> "/" <> method

    case Req.get(url,
           headers: [{"Authorization", "Bearer " <> token}],
           params: params,
           connect_options: [timeout: 30_000]
         ) do
      {:ok, %{status: 200, body: %{"ok" => true} = body}} ->
        {:ok, body}

      {:ok, %{status: 200, body: %{"ok" => false, "error" => slack_err}}} ->
        {:error, {:slack_api_error, method, slack_err}}

      {:ok, %{status: status, body: body}} ->
        {:error, {:slack_http_status, status, body}}

      {:error, reason} ->
        {:error, {:slack_request_failed, reason}}
    end
  end

  defp slack_post(token, method, payload) do
    config = Config.get()
    url = config.slack_endpoint <> "/" <> method

    case Req.post(url,
           headers: [
             {"Authorization", "Bearer " <> token},
             {"Content-Type", "application/json; charset=utf-8"}
           ],
           json: payload,
           connect_options: [timeout: 30_000]
         ) do
      {:ok, %{status: 200, body: %{"ok" => true} = body}} ->
        {:ok, body}

      {:ok, %{status: 200, body: %{"ok" => false, "error" => slack_err}}} ->
        {:error, {:slack_api_error, method, slack_err}}

      {:ok, %{status: status, body: body}} ->
        {:error, {:slack_http_status, status, body}}

      {:error, reason} ->
        {:error, {:slack_request_failed, reason}}
    end
  end

  defp bot_token do
    case Config.get().slack_bot_token do
      nil -> {:error, :missing_slack_token}
      token -> {:ok, token}
    end
  end
end
