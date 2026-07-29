defmodule IxMcp.SessionWatch do
  @moduledoc """
  This instance's liveness heartbeat and shared-bus delivery loop (#3881).
  Every tick -- seconds, because messaging is a conversation -- it stamps
  the session's `last_seen_at` in the shared actions.db and sweeps two
  per-instance watermarks: `session_messages` rows addressed to this
  session (or broadcast) arrive as `source="sessions"` channel
  notifications naming the sender, and `request_events` (#3883) as
  `source="requests"` notifications with `event` posted/claimed/done -- the
  request feed hears every session on the host, this instance's own moves
  included (harmless: the actor already knows, and one notification tells
  its transcript too).

  Deliberately its own GenServer rather than a bolt-on to `IxMcp.IssueWatch`:
  that loop's 60s cadence is set by GitHub polling politeness, while this
  one talks only to the local SQLite, where seconds cost nothing. It starts
  only alongside the stdio transport (`IxMcp.Application`), so `mix test`
  and IEx sessions neither heartbeat nor deliver.

  Both watermarks start at the newest row on boot. For messages that skips
  only broadcasts from before this instance existed -- old news, the claim
  feed's exact call (#3880) -- because addressed mail cannot predate its
  address: sessions rows are per instance and never reused, so a message to
  this session's id can only be written after this instance created the
  row. For requests it skips pre-boot events the same way the claim feed
  did, while `Requests.list()` still shows any standing open work.
  """

  use GenServer

  alias IxMcp.ActionLog
  alias IxMcp.MCP.Notifier
  alias IxMcp.Session

  @interval_ms 3_000
  # The channel event should carry the message, not become the whole
  # transcript: an essay still arrives truncated, with the sender's id to
  # ask for the rest.
  @max_body_bytes 2_000

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, __MODULE__))
  end

  @impl true
  def init(opts) do
    action_log = Keyword.get(opts, :action_log, ActionLog)
    # Resolving the id creates the sessions row when nothing acted yet:
    # starting the watch is what makes this instance a directory citizen.
    session_id = Keyword.get_lazy(opts, :session_id, fn -> Session.ids().session_id end)

    state = %{
      action_log: action_log,
      session_id: session_id,
      interval_ms: Keyword.get(opts, :interval_ms, @interval_ms),
      messages_cursor: ActionLog.last_session_message_id(action_log),
      requests_cursor: ActionLog.last_request_event_id(action_log)
    }

    # Stamp now, not one tick from now: the instance is live from boot.
    :ok = ActionLog.heartbeat_session(session_id, action_log)
    {:ok, schedule(state)}
  end

  @impl true
  def handle_info(:tick, state) do
    :ok = ActionLog.heartbeat_session(state.session_id, state.action_log)
    {:noreply, state |> sweep() |> schedule()}
  end

  defp schedule(state) do
    Process.send_after(self(), :tick, state.interval_ms)
    state
  end

  # Deliver everything past each watermark and advance it as one fold: the
  # cursor stays monotone with no empty-sweep special case. One sweep shape
  # for both feeds -- the fetch and the announcement differ, the cursor
  # discipline must not.
  defp sweep(state) do
    state
    |> sweep_feed(
      :messages_cursor,
      &ActionLog.session_messages_after(&1, state.session_id, state.action_log),
      &announce/1
    )
    |> sweep_feed(
      :requests_cursor,
      &ActionLog.request_events_after(&1, state.action_log),
      &announce_request/1
    )
  end

  defp sweep_feed(state, cursor_key, fetch, announce) do
    state
    |> Map.fetch!(cursor_key)
    |> fetch.()
    |> Enum.reduce(state, fn row, acc ->
      announce.(row)
      Map.update!(acc, cursor_key, &max(&1, row.id))
    end)
  end

  defp announce_request(event) do
    label = event.session || "##{event.session_id || "?"}"

    # Issue rows ensured by a pickup use the ref as their title; only name
    # the ref separately when a poster gave the request a real one.
    what =
      if event.ref && event.ref != event.title,
        do: "#{event.title} (#{event.ref})",
        else: event.title

    detail =
      case event.event do
        :posted ->
          body = if event.body, do: "\n#{String.slice(event.body, 0, @max_body_bytes)}", else: ""
          "#{body}\npickup: Requests.pickup(#{event.request_id})"

        _claimed_or_done ->
          ""
      end

    meta = %{
      "source" => "requests",
      "event" => Atom.to_string(event.event),
      "request" => Integer.to_string(event.request_id),
      "kind" => Atom.to_string(event.kind),
      "session" => label,
      "level" => "info"
    }

    # An adhoc request has no ref; the key is omitted rather than sent empty.
    meta = if event.ref, do: Map.put(meta, "ref", event.ref), else: meta

    Notifier.channel(
      "request #{event.event}: ##{event.request_id} #{what} by session #{label}#{detail}",
      meta
    )
  end

  defp announce(message) do
    from = label(message)
    scope = if message.to_session, do: "you", else: "broadcast"

    Notifier.channel(
      "message from session #{from} (to #{scope}):\n" <>
        "#{String.slice(message.body, 0, @max_body_bytes)}\n" <>
        "reply: Sessions.send(#{message.from_session}, \"...\")",
      %{
        "source" => "sessions",
        "from" => from,
        "from_id" => Integer.to_string(message.from_session),
        "to" => scope,
        "level" => "info"
      }
    )
  end

  defp label(%{from: nil, from_session: id}), do: "##{id}"
  defp label(%{from: name, from_session: id}), do: "#{name} (##{id})"
end
