defmodule IxMcp.MCP.Notifier do
  @moduledoc """
  Fan-out point for server-initiated MCP notifications. Session wakes ride
  the Claude Code channel contract: `notifications/claude/channel` events,
  paired with the experimental `claude/channel` capability the server
  declares at initialize. Claude Code receives `notifications/message` (MCP
  logging) but never surfaces it, so nothing user-facing may depend on that
  method (#3785).

  Notifications derive from durable transitions (#3839). A terminal job
  transition inserts a row into the `outbox` table (part of the same atomic
  write as the transition); `publish/1` delivers it to a connected transport
  and marks it acked. When no transport is connected -- the second silence
  the incident exposed, where events fired while nothing was listening were
  simply dropped -- the row stays unacked and `register/1` replays every
  unacked row as one digest channel message the moment a transport (re)joins.
  """

  use GenServer

  alias IxMcp.ActionLog
  alias IxMcp.Session

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @doc """
  Register a transport. Any outbox notifications that fired while no
  transport was connected are replayed at once as a single digest.
  """
  @spec register(pid()) :: :ok
  def register(transport) when is_pid(transport) do
    GenServer.cast(__MODULE__, {:register, transport})
  end

  @doc """
  Deliver a terminal-transition notification. Sent to every connected
  transport and acked; with no transport connected it is a no-op and the
  outbox row waits for `register/1` to replay it.
  """
  @spec publish(ActionLog.outbox()) :: :ok
  def publish(outbox) do
    case Process.whereis(__MODULE__) do
      nil -> :ok
      pid -> GenServer.cast(pid, {:publish, outbox})
    end
  end

  @doc """
  Push one event into the connected Claude session. `content` becomes the
  body of the `<channel>` tag the client injects; each `meta` entry becomes
  a tag attribute, so values must be short scalars.
  """
  @spec channel(String.t(), %{optional(String.t()) => String.t() | number()}) :: :ok
  def channel(content, meta) do
    notify("notifications/claude/channel", %{"content" => content, "meta" => meta})
  end

  @spec notify(String.t(), map()) :: :ok
  def notify(method, params) do
    case Process.whereis(__MODULE__) do
      nil -> :ok
      pid -> GenServer.cast(pid, {:notify, method, params})
    end
  end

  @impl true
  def init(_), do: {:ok, %{transports: []}}

  @impl true
  def handle_cast({:register, transport}, state) do
    Process.monitor(transport)
    state = %{state | transports: [transport | state.transports]}
    # A connecting transport is proof of life: stamp the session-directory
    # heartbeat (#3881) here, without waiting for the first watch tick.
    ActionLog.heartbeat_session(Session.ids().session_id)
    replay_unacked([transport])
    {:noreply, state}
  end

  def handle_cast({:publish, outbox}, state) do
    # Deliver before acking (#3874). The old order claimed the ack first as
    # the dedup arbiter, but the log's client API now retries a call whose
    # server died mid-request: an ack that committed without a reply looks
    # already-claimed on retry, and skipping delivery on that evidence
    # would silence the notification permanently -- the exact silence the
    # outbox exists to prevent. Publish and register-replay both run in
    # this process, so checking the row is still unacked before delivering
    # keeps the same-finish dedup (#3839) race-free; the residual worst
    # case is a duplicate announce, which beats a lost one. With no
    # transport connected we neither deliver nor ack -- the row waits for
    # replay.
    if state.transports != [] and unacked?(outbox.id) do
      {content, meta} = render(outbox)

      deliver(state.transports, "notifications/claude/channel", %{
        "content" => content,
        "meta" => meta
      })

      ack_all([outbox.id])
    end

    {:noreply, state}
  end

  def handle_cast({:notify, method, params}, state) do
    deliver(state.transports, method, params)
    {:noreply, state}
  end

  @impl true
  def handle_info({:DOWN, _ref, :process, pid, _reason}, state) do
    {:noreply, %{state | transports: List.delete(state.transports, pid)}}
  end

  # Replay every unacked outbox row as one digest so a reconnecting session
  # sees "while you were away: ..." rather than nothing (#3839). Scoped to
  # this instance's session, so a transport connecting here never drains a
  # sibling instance's notifications out of the shared database.
  defp replay_unacked(transports) do
    case unacked_rows(Session.ids().session_id) do
      [] ->
        :ok

      rows ->
        lines = Enum.map_join(rows, "\n", fn row -> "- " <> line_of(row) end)
        content = "while you were away, #{length(rows)} job(s) finished:\n" <> lines
        meta = %{"source" => "jobs", "replay" => length(rows)}

        deliver(transports, "notifications/claude/channel", %{
          "content" => content,
          "meta" => meta
        })

        rows |> Enum.map(& &1.id) |> ack_all()
    end
  end

  defp unacked_rows(session_id) do
    ActionLog.unacked_outbox(session_id)
  catch
    :exit, _reason -> []
  end

  defp ack_all(ids) do
    ActionLog.ack_outbox(ids)
  catch
    :exit, _reason -> 0
  end

  # The ledger interactions must never crash the notifier: its state is the
  # live transport registry, and losing it silences every future
  # notification until the transports reconnect (#3874). A row whose ack
  # was lost stays unacked and may replay as a duplicate later -- accepted.
  defp unacked?(outbox_id) do
    Enum.any?(ActionLog.unacked_outbox(), &(&1.id == outbox_id))
  catch
    :exit, _reason -> false
  end

  defp deliver(transports, method, params) do
    message = %{"jsonrpc" => "2.0", "method" => method, "params" => params}
    Enum.each(transports, fn pid -> send(pid, {:mcp_send, message}) end)
  end

  defp render(outbox) do
    status = Atom.to_string(outbox.status)

    content =
      "job #{outbox.job_id} (#{outbox.intent || "no intent"}) finished: #{status} " <>
        "in #{elapsed_s(outbox.elapsed_ms)}s\n#{String.slice(outbox.result || "", 0, 2_000)}"

    meta = %{"source" => "jobs", "job" => outbox.job_id, "status" => status}
    {content, meta}
  end

  defp line_of(outbox) do
    status = Atom.to_string(outbox.status)

    "job #{outbox.job_id} (#{outbox.intent || "no intent"}): #{status} in #{elapsed_s(outbox.elapsed_ms)}s"
  end

  defp elapsed_s(nil), do: 0.0
  defp elapsed_s(ms), do: ms / 1000
end
