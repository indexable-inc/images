defmodule IxMcp.SessionWatch do
  @moduledoc """
  This instance's liveness heartbeat and message delivery loop (#3881).
  Every tick -- seconds, because messaging is a conversation -- it stamps
  the session's `last_seen_at` in the shared actions.db and sweeps
  `session_messages` past a per-instance watermark, pushing each row
  addressed to this session (or broadcast) into the agent's context as a
  `source="sessions"` channel notification naming the sender.

  Deliberately its own GenServer rather than a bolt-on to `IxMcp.IssueWatch`:
  that loop's 60s cadence is set by GitHub polling politeness, while this
  one talks only to the local SQLite, where seconds cost nothing. It starts
  only alongside the stdio transport (`IxMcp.Application`), so `mix test`
  and IEx sessions neither heartbeat nor deliver.

  The watermark starts at the newest message on boot. That skips only
  broadcasts from before this instance existed -- old news, the claim
  feed's exact call (#3880) -- because addressed mail cannot predate its
  address: sessions rows are per instance and never reused, so a message to
  this session's id can only be written after this instance created the row.
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
      cursor: ActionLog.last_session_message_id(action_log)
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

  # Deliver everything past the watermark and advance it as one fold: the
  # cursor stays monotone with no empty-sweep special case.
  defp sweep(state) do
    state.cursor
    |> ActionLog.session_messages_after(state.session_id, state.action_log)
    |> Enum.reduce(state, fn message, acc ->
      announce(message)
      %{acc | cursor: max(acc.cursor, message.id)}
    end)
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
        "from_id" => message.from_session,
        "to" => scope,
        "level" => "info"
      }
    )
  end

  defp label(%{from: nil, from_session: id}), do: "##{id}"
  defp label(%{from: name, from_session: id}), do: "#{name} (##{id})"
end
