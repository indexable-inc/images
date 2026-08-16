defmodule IxMcp.Fleet.WarningsWatch do
  @moduledoc """
  The opt-in relay from `FleetMesh.Engine` edges to the kernel's channel.

  Off by default, deliberately. Every session on this kernel shares one
  channel, so one watcher covers everyone, and a second would double every
  line. The singleton name is the dedup: a second `start/1` returns
  `{:error, {:already_started, _}}` and `Fleet.watch_warnings/1` turns that
  into "already watching, enabled by X" instead of a duplicate.

  What it adds over the announce poller: transitions. The poller announces
  new red hits (fingerprint-deduped) but nothing ever says a condition went
  back to green, or that a check stopped being able to run. An edge watcher
  hears all three.
  """

  use GenServer

  alias FleetMesh.Engine
  alias IxMcp.MCP.Notifier

  @doc "Start watching. `requested_by` names who asked, for the dedup answer."
  @spec start(String.t()) :: GenServer.on_start()
  def start(requested_by) do
    GenServer.start(__MODULE__, requested_by, name: __MODULE__)
  end

  @doc "Stop watching. `:ok` even when nothing was watching."
  @spec stop() :: :ok
  def stop do
    case Process.whereis(__MODULE__) do
      nil -> :ok
      pid -> GenServer.stop(pid)
    end
  end

  @doc "Who enabled the current watch, or nil when nothing is watching."
  @spec watcher() :: String.t() | nil
  def watcher do
    case Process.whereis(__MODULE__) do
      nil -> nil
      pid -> GenServer.call(pid, :watcher)
    end
  end

  @impl true
  def init(requested_by) do
    {:ok, _already} = Engine.subscribe(Engine, %{warnings_watch: requested_by})
    {:ok, %{requested_by: requested_by}}
  end

  @impl true
  def handle_call(:watcher, _from, state), do: {:reply, state.requested_by, state}

  @impl true
  def handle_info({:fleet_snapshot, _snapshot}, state), do: {:noreply, state}

  def handle_info({:fleet_edge, id, from, to, detail}, state) do
    Notifier.channel(render(id, from, to, detail), %{
      "source" => "fleet_warning_edge",
      "severity" => severity(to),
      "condition" => Atom.to_string(id)
    })

    {:noreply, state}
  end

  defp severity(:red), do: "failure"
  defp severity(:unknown), do: "warning"
  defp severity(:green), do: "info"

  defp render(id, from, to, detail) do
    "fleet condition #{id}: #{from} -> #{to}" <> detail_line(to, detail)
  end

  defp detail_line(:green, _detail), do: " (recovered)"

  defp detail_line(:red, hits) when is_list(hits) do
    "\n" <> Enum.map_join(hits, "\n", &("- " <> &1.summary))
  end

  defp detail_line(:unknown, reason), do: " (check could not run: #{inspect(reason)})"
  defp detail_line(_state, _detail), do: ""
end
