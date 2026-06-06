defmodule SymphonyElixir.Triggers.Slack do
  @moduledoc """
  Polls Slack for completed huddles in channels referenced by any `.sym`
  workflow whose `trigger.kind = :slack_huddle_completed`, and starts one
  IR run per fresh huddle.

  A huddle is "completed" when its `huddle_thread` message has
  `room.has_ended = true` and the Slack-AI summary has reached
  `room.recording.summary_status = "complete"`. We dedupe on the
  message timestamp by checking for an existing IR run (any status) that
  references the same `message_ts`.

  Polls every `Config.slack_poll_ms`. Stays idle when
  `SLACK_BOT_OAUTH_TOKEN` is absent. Sibling to the Linear webhook.
  """

  use GenServer
  require Logger

  alias SymphonyElixir.{Config, Slack.Client, WorkflowCatalog}
  alias SymphonyElixir.Runtime.Ingress

  @history_window_seconds 86_400 * 2

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    config = Config.get()
    schedule_poll(config.slack_poll_ms)
    {:ok, %{poll_ms: config.slack_poll_ms, channel_ids: %{}}}
  end

  @impl true
  def handle_info(:poll, state) do
    state = poll_once(state)
    schedule_poll(state.poll_ms)
    {:noreply, state}
  end

  defp schedule_poll(ms), do: Process.send_after(self(), :poll, ms)

  defp poll_once(state) do
    if is_nil(Config.get().slack_bot_token) do
      state
    else
      WorkflowCatalog.for_trigger_kind(:slack_huddle_completed)
      |> Enum.reduce(state, &poll_workflow/2)
    end
  end

  defp poll_workflow(entry, state) do
    channel_name = entry.trigger.channel

    case resolve_channel_id(channel_name, state.channel_ids) do
      {:ok, channel_id, new_cache} ->
        case Client.conversations_history(channel_id,
               oldest: oldest_window(),
               limit: 50
             ) do
          {:ok, messages} ->
            Enum.each(messages, fn msg ->
              maybe_enqueue(channel_name, channel_id, msg)
            end)

            %{state | channel_ids: new_cache}

          {:error, reason} ->
            Logger.warning("Slack huddle poll for workflow=#{entry.name} channel=#{channel_name} failed: #{inspect(reason)}")

            %{state | channel_ids: new_cache}
        end

      {:error, reason} ->
        Logger.warning("Slack channel resolution for workflow=#{entry.name} channel=#{channel_name} failed: #{inspect(reason)}")

        state
    end
  end

  defp resolve_channel_id(channel_name, cache) do
    case Map.fetch(cache, channel_name) do
      {:ok, id} ->
        {:ok, id, cache}

      :error ->
        case Client.resolve_channel_id(channel_name) do
          {:ok, id} -> {:ok, id, Map.put(cache, channel_name, id)}
          {:error, _} = err -> err
        end
    end
  end

  defp oldest_window do
    System.system_time(:second) - @history_window_seconds
  end

  defp maybe_enqueue(channel_name, channel_id, %{"subtype" => "huddle_thread"} = msg) do
    room = Map.get(msg, "room", %{})
    recording = Map.get(room, "recording", %{})

    cond do
      Map.get(room, "has_ended") != true ->
        :ok

      Map.get(recording, "summary_status") != "complete" ->
        :ok

      true ->
        message_ts = Map.get(msg, "ts")

        cond do
          is_nil(message_ts) ->
            :ok

          already_seen?(message_ts) ->
            :ok

          true ->
            trigger = build_trigger(channel_name, channel_id, msg, room, recording)
            enqueue(trigger)
        end
    end
  end

  defp maybe_enqueue(_channel_name, _channel_id, _msg), do: :ok

  defp build_trigger(channel_name, channel_id, msg, room, _recording) do
    files = Map.get(msg, "files", [])

    canvas_file_id =
      Enum.find_value(files, fn f -> if f["filetype"] == "quip", do: f["id"] end)

    transcript_file_id =
      Map.get(room, "huddle_transcript_file_id") ||
        Enum.find_value(files, fn f ->
          if f["filetype"] == "huddle_transcript", do: f["id"]
        end)

    %{
      kind: :slack_huddle_completed,
      channel: channel_name,
      channel_id: channel_id,
      message_ts: Map.get(msg, "ts"),
      date_start: Map.get(room, "date_start") || 0,
      date_end: Map.get(room, "date_end") || 0,
      canvas_file_id: canvas_file_id,
      transcript_file_id: transcript_file_id,
      permalink: Map.get(msg, "permalink"),
      participants: Map.get(room, "participant_history", []) || []
    }
  end

  defp enqueue(trigger) do
    case Ingress.start_by_trigger(trigger) do
      {:ok, started} ->
        Logger.info("Started runs=#{Enum.map_join(started, ",", & &1.run_id)} for huddle channel=#{trigger.channel} ts=#{trigger.message_ts}")

      {:error, reason} ->
        Logger.warning("Failed to start huddle run channel=#{trigger.channel} ts=#{trigger.message_ts}: #{inspect(reason)}")
    end
  end

  # Dedupe across every IR run, not per workflow: a completed huddle should
  # fire each interested workflow once, and a second poll of the same
  # `message_ts` must start nothing new. A run started for this huddle
  # carries the `message_ts` on its trigger, so its presence (any status)
  # is the watermark.
  defp already_seen?(message_ts) do
    Ingress.seen_trigger?(fn
      {_status, %{kind: :slack_huddle_completed, message_ts: ts}} -> ts == message_ts
      {_status, _trigger} -> false
    end)
  end
end
