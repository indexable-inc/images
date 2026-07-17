defmodule IxMcp.MCP.Notifier do
  @moduledoc """
  Fan-out point for server-initiated MCP notifications (the "channel"): job
  completions and failures are pushed to every connected transport, carrying
  the full crash reason -- on the BEAM the exit reason is already a crash
  report with state, so nothing has to be reconstructed after the fact.

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

  @spec job_finished(IxMcp.Jobs.Job.summary()) :: :ok
  def job_finished(summary) do
    notify("notifications/message", %{
      "level" => level_for(summary.status),
      "logger" => "ix_mcp.jobs",
      "data" => %{
        "event" => "job_finished",
        "job" => summary.id,
        "status" => Atom.to_string(summary.status),
        "intent" => summary.intent,
        "elapsed_s" => summary.elapsed_s,
        "result" => summary.result
      }
    })
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

  defp level_for(:done), do: "info"
  defp level_for(_), do: "error"
end
