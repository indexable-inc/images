defmodule SymphonyElixir.IR.RunNotifier do
  @moduledoc """
  Posts one Slack summary to `config.slack_notify_channel` when an IR run
  reaches a terminal state.

  Best-effort: a Slack failure is logged and never changes the run outcome.
  The notifier reinstates the post-run notification that was dropped when the
  YAML/DAG runtime and its `RunNotifier` were deleted in the IR cutover
  (originally #111); the `SYMPHONY_SLACK_NOTIFY_CHANNEL` knob had no consumer
  in between, so the channel went silent.

  Cron notifications follow a configurable policy so the high-frequency
  internal dispatchers do not flood the channel while real failures and
  selected digests stay visible. A failed cron run posts when
  `config.slack_notify_cron_failures` is set (default true); a succeeded
  cron run posts when its workflow name is listed in
  `config.slack_notify_cron_workflows`, or when that list contains the
  wildcard `"*"` to post every cron success. Set the allowlist to `"*"` to
  turn the whole channel back on without enumerating every workflow; list
  specific names to surface only a few. The policy reads workflow names from
  config, never a literal in source, so `elixir/lib/` stays pack-agnostic.
  Non-cron terminal runs always notify.
  """

  require Logger

  alias SymphonyElixir.Codex.Provision
  alias SymphonyElixir.Config
  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.Slack.Client, as: SlackClient

  @doc """
  Post the terminal summary for `graph`. No-op when the run should not notify
  (non-terminal, cancelled, or a cron run the policy suppresses) or when
  Slack is not configured.
  """
  @spec notify_finished(RunGraph.t()) :: :ok
  def notify_finished(%RunGraph{} = graph) do
    config = Config.get()

    cond do
      not notify?(graph, config) ->
        :ok

      is_nil(config.slack_bot_token) or is_nil(config.slack_notify_channel) ->
        :ok

      true ->
        payload = build_payload(graph, config.room.registry_url)

        case SlackClient.post_message(config.slack_notify_channel, payload) do
          {:ok, _body} ->
            :ok

          {:error, reason} ->
            Logger.warning("RunNotifier Slack post failed for #{graph.run_id}: #{inspect(reason)}")
            :ok
        end
    end
  end

  @doc """
  Whether a finished run should produce a Slack notification. Only real
  outcomes (`:succeeded`/`:failed`) notify. Non-cron runs always notify; a
  cron run defers to the per-workflow policy in `config` so scheduled
  dispatchers stay out of the channel unless a failure or an allowlisted
  workflow warrants it.
  """
  @spec notify?(RunGraph.t(), Config.t()) :: boolean()
  def notify?(%RunGraph{status: status}, _config) when status not in [:succeeded, :failed], do: false

  def notify?(%RunGraph{trigger: trigger} = graph, %Config{} = config) do
    if trigger_kind(trigger) == :cron, do: cron_notify?(graph, config), else: true
  end

  # A cron failure posts when failure notifications are enabled; a cron
  # success posts for an allowlisted workflow, or for every workflow when the
  # allowlist is the wildcard "*", so digests (or the whole channel) can be
  # surfaced while babysit-dispatch and other tight-interval runs stay quiet by
  # default (ENG-2012, indexable-inc/symphony#242).
  defp cron_notify?(%RunGraph{status: :failed}, %Config{slack_notify_cron_failures: notify_failures}) do
    notify_failures
  end

  defp cron_notify?(%RunGraph{} = graph, %Config{slack_notify_cron_workflows: workflows}) do
    "*" in workflows or workflow_name(graph.run_id) in workflows
  end

  # `room_base_url` is the central room UI origin (`config.room.registry_url`),
  # the same room.ix.dev the run's room-server registers its backend with. The
  # "Run details" button deep-links into that UI's transcript for the run, not
  # into this dashboard.
  @doc false
  @spec build_payload(RunGraph.t(), String.t() | nil) :: map()
  def build_payload(%RunGraph{} = graph, room_base_url) do
    workflow = workflow_name(graph.run_id)
    status = graph.status
    summary = summary(graph, workflow)
    header_text = "#{status_icon(status)} #{workflow} #{status_word(status)}"

    blocks =
      [
        header(header_text),
        section(summary),
        context(context_text(graph)),
        actions(graph, room_base_url)
      ]
      |> Enum.reject(&is_nil/1)

    %{
      "text" => fallback_text(workflow, status, summary),
      "unfurl_links" => false,
      "unfurl_media" => false,
      "blocks" => blocks
    }
  end

  # run_id is "<workflow-slug>-<ms>-<unique>" (Ingress.generate_run_id/1), so
  # the display name is the id with that numeric suffix stripped.
  @doc false
  @spec workflow_name(String.t()) :: String.t()
  def workflow_name(run_id) when is_binary(run_id) do
    case String.replace(run_id, ~r/-\d+-\d+$/, "") do
      "" -> run_id
      name -> name
    end
  end

  defp summary(%RunGraph{status: :failed} = graph, workflow) do
    case failed_node_ids(graph) do
      [] -> "Run #{code(workflow)} failed."
      ids -> "Run #{code(workflow)} failed in #{Enum.map_join(ids, ", ", &code/1)}."
    end
  end

  defp summary(%RunGraph{} = graph, workflow) do
    "Completed #{code(workflow)} (#{node_breakdown(graph)})."
  end

  defp failed_node_ids(%RunGraph{nodes: nodes}) do
    for {id, node} <- nodes, node.state == :failed, do: id
  end

  defp node_breakdown(%RunGraph{nodes: nodes}) do
    nodes
    |> Map.values()
    |> Enum.frequencies_by(& &1.state)
    |> Enum.map_join(", ", fn {state, count} -> "#{count} #{state}" end)
  end

  defp context_text(%RunGraph{} = graph) do
    [code(graph.run_id), trigger_label(graph.trigger), Atom.to_string(graph.status), duration(graph)]
    |> Enum.reject(&(&1 in [nil, ""]))
    |> Enum.join(" - ")
  end

  defp actions(%RunGraph{} = graph, room_base_url) do
    buttons =
      []
      |> maybe_add_linear_button(graph.trigger)
      |> maybe_add_run_button(graph, room_base_url)
      |> Enum.reverse()

    case buttons do
      [] -> nil
      _ -> %{"type" => "actions", "elements" => buttons}
    end
  end

  defp maybe_add_linear_button(buttons, trigger) when is_map(trigger) do
    case {trigger_field(trigger, :url), trigger_field(trigger, :identifier)} do
      {url, id} when is_binary(url) and url != "" and is_binary(id) and id != "" ->
        [button(id, url, "primary") | buttons]

      _ ->
        buttons
    end
  end

  defp maybe_add_linear_button(buttons, _trigger), do: buttons

  # The run's transcript lives on room.ix.dev, not in this dashboard: every
  # agent turn streams into the run's room-server, which registers a backend
  # with the central room UI under `Provision.backend_id(run_id, "room")`.
  # Deep-link to that backend's most recent thread when the run opened one, else
  # to the room root. `base_url` is `config.room.registry_url`; with no room
  # configured there is nothing to point at, so the button is omitted rather
  # than emitting a dead link.
  defp maybe_add_run_button(buttons, %RunGraph{} = graph, base_url) when is_binary(base_url) and base_url != "" do
    [button("Run details", room_run_url(base_url, graph), nil) | buttons]
  end

  defp maybe_add_run_button(buttons, _graph, _base_url), do: buttons

  # The room client is a hash router: `#/s/<server_id>/t/<thread_id>` opens a
  # thread on a backend, `/` lands on the room. `server_id` is the registered
  # backend id; segments are encoded the way the client's encodeURIComponent
  # links are, so its decodeURIComponent parse recovers the raw ids.
  defp room_run_url(base_url, %RunGraph{run_id: run_id} = graph) do
    base = String.trim_trailing(base_url, "/")
    server = URI.encode(Provision.backend_id(run_id, "room"), &URI.char_unreserved?/1)

    case primary_thread_id(graph) do
      nil -> base <> "/"
      thread_id -> base <> "/#/s/" <> server <> "/t/" <> URI.encode(thread_id, &URI.char_unreserved?/1)
    end
  end

  # The latest agent thread the run opened on its room-server, ordered by
  # attempt start. A run with no agent turn that reached the engine has no
  # thread, so the link falls back to the room root.
  defp primary_thread_id(%RunGraph{nodes: nodes}) do
    nodes
    |> Map.values()
    |> Enum.filter(&(&1.kind == :agent))
    |> Enum.flat_map(& &1.attempts)
    |> Enum.filter(fn attempt -> is_binary(attempt.thread_id) and attempt.thread_id != "" end)
    |> Enum.sort_by(& &1.started_at, {:desc, DateTime})
    |> case do
      [%{thread_id: thread_id} | _] -> thread_id
      [] -> nil
    end
  end

  defp fallback_text(workflow, status, summary) do
    # Slack flattens blocks to this text in push/desktop/sidebar previews, and
    # mrkdwn does not render there, so keep it plain.
    "Symphony: #{workflow} #{status_word(status)} - #{plain(summary)}"
    |> truncate(200)
  end

  defp header(text) do
    # header blocks only accept plain_text and cap at 150 chars.
    %{"type" => "header", "text" => %{"type" => "plain_text", "text" => truncate(text, 150), "emoji" => true}}
  end

  defp section(text) do
    %{"type" => "section", "text" => %{"type" => "mrkdwn", "text" => text}}
  end

  defp context(text) do
    %{"type" => "context", "elements" => [%{"type" => "mrkdwn", "text" => text}]}
  end

  defp button(text, url, style) do
    %{"type" => "button", "text" => %{"type" => "plain_text", "text" => text, "emoji" => true}, "url" => url}
    |> maybe_put_style(style)
  end

  defp maybe_put_style(button, nil), do: button
  defp maybe_put_style(button, style), do: Map.put(button, "style", style)

  defp trigger_kind(%{kind: kind}) when is_atom(kind), do: kind
  defp trigger_kind(%{"kind" => kind}) when is_binary(kind), do: kind_atom(kind)
  defp trigger_kind(_), do: nil

  defp trigger_field(trigger, key) do
    Map.get(trigger, key) || Map.get(trigger, Atom.to_string(key))
  end

  # Trigger maps reach us with atom keys in-process; a string "kind" only
  # appears after a store round-trip. Match the known kinds rather than
  # minting atoms from arbitrary input.
  defp kind_atom("cron"), do: :cron
  defp kind_atom("linear"), do: :linear
  defp kind_atom(_other), do: :other

  defp trigger_label(trigger) do
    case trigger_kind(trigger) do
      :cron -> "Cron trigger"
      :linear -> "Linear trigger"
      :manual -> "Manual trigger"
      :github_pr_label -> "GitHub trigger"
      :slack_app_mention -> "Slack mention trigger"
      :slack_huddle_completed -> "Slack huddle trigger"
      _ -> "Symphony"
    end
  end

  defp duration(%RunGraph{created_at: %DateTime{} = created, updated_at: %DateTime{} = updated}) do
    seconds = max(DateTime.diff(updated, created, :second), 0)

    cond do
      seconds >= 3600 -> "#{div(seconds, 3600)}h #{div(rem(seconds, 3600), 60)}m"
      seconds >= 60 -> "#{div(seconds, 60)}m #{rem(seconds, 60)}s"
      true -> "#{seconds}s"
    end
  end

  defp duration(_graph), do: nil

  defp status_word(:succeeded), do: "finished"
  defp status_word(:failed), do: "failed"
  defp status_word(status), do: Atom.to_string(status)

  defp status_icon(:succeeded), do: ":white_check_mark:"
  defp status_icon(:failed), do: ":x:"
  defp status_icon(_status), do: ":information_source:"

  defp plain(text) when is_binary(text) do
    text |> String.replace(~r/[`*_~]/, "") |> String.replace(~r/\s+/, " ") |> String.trim()
  end

  defp code(text), do: "`" <> to_string(text) <> "`"

  defp truncate(text, limit) when byte_size(text) <= limit, do: text

  defp truncate(text, limit) do
    text |> String.slice(0, limit - 1) |> String.trim() |> Kernel.<>("...")
  end
end
