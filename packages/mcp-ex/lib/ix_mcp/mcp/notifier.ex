defmodule IxMcp.MCP.Notifier do
  @moduledoc """
  Fan-out point for server-initiated MCP notifications. Session wakes ride
  the Claude Code channel contract: `notifications/claude/channel` events,
  paired with the experimental `claude/channel` capability the server
  declares at initialize. Claude Code receives `notifications/message` (MCP
  logging) but never surfaces it, so nothing user-facing may depend on that
  method (#3785).

  Transports register themselves; when none is connected (tests, IEx),
  notifying is a no-op rather than an error.
  """

  use GenServer

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @spec register(pid()) :: :ok
  def register(transport) when is_pid(transport) do
    GenServer.cast(__MODULE__, {:register, transport})
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

  @spec job_finished(IxMcp.Jobs.Job.summary()) :: :ok
  def job_finished(summary) do
    status = Atom.to_string(summary.status)

    channel(
      "job #{summary.id} (#{summary.intent || "no intent"}) finished: #{status} " <>
        "in #{summary.elapsed_s}s\n#{String.slice(summary.result || "", 0, 2_000)}",
      %{"source" => "jobs", "job" => summary.id, "status" => status}
    )
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
    {:noreply, %{state | transports: [transport | state.transports]}}
  end

  def handle_cast({:notify, method, params}, state) do
    message = %{"jsonrpc" => "2.0", "method" => method, "params" => params}
    Enum.each(state.transports, fn pid -> send(pid, {:mcp_send, message}) end)
    {:noreply, state}
  end

  @impl true
  def handle_info({:DOWN, _ref, :process, pid, _reason}, state) do
    {:noreply, %{state | transports: List.delete(state.transports, pid)}}
  end

end
