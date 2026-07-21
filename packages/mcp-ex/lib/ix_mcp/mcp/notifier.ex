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
    replay_unacked([transport])
    {:noreply, state}
  end

  def handle_cast({:publish, outbox}, state) do
    {content, meta} = render(outbox)

    deliver(state.transports, "notifications/claude/channel", %{
      "content" => content,
      "meta" => meta
    })

    # Delivered to a live transport: ack it. Otherwise leave it for replay.
    if state.transports != [], do: ActionLog.ack_outbox([outbox.id])
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
  # sees "while you were away: ..." rather than nothing (#3839).
  defp replay_unacked(transports) do
    case ActionLog.unacked_outbox() do
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

        ActionLog.ack_outbox(Enum.map(rows, & &1.id))
    end
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
