defmodule IxMcp.Jobs.Reaper do
  @moduledoc """
  The single reaper (#3839). Every job control process registers here at
  start (`watch/2`); the reaper monitors it and, if it dies without having
  reported a terminal transition, writes a `killed` transition to the durable
  ledger from outside and delivers its notification.

  This generalizes the #3538 History monitor: before, only an in-memory
  history row got corrected on a job's death, and only for a poisoned-buffer
  crash. Now the reaper closes the gap for *any* way a control process can
  die unreported -- a hard `Process.exit(pid, :kill)` (which trapping exits
  in the job cannot catch), the kernel-internal crash that took out four
  sessions in the incident -- by driving the ledger terminal and notifying.
  The transition is idempotent (`ActionLog.finish_job` no-ops a job that
  already went terminal), so a job that reported cleanly and then died
  naturally costs the reaper nothing.
  """

  use GenServer

  alias IxMcp.ActionLog
  alias IxMcp.MCP.Notifier

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @doc "Monitor a job control process so its unreported death is finalized."
  @spec watch(String.t(), pid()) :: :ok
  def watch(id, pid) do
    GenServer.cast(__MODULE__, {:watch, id, pid})
  end

  @doc "A job reported a terminal transition itself; stop guarding it."
  @spec reported(String.t()) :: :ok
  def reported(id) do
    GenServer.cast(__MODULE__, {:reported, id})
  end

  @impl true
  def init(_) do
    {:ok, %{refs: %{}, ids: %{}}}
  end

  @impl true
  def handle_cast({:watch, id, pid}, state) do
    ref = Process.monitor(pid)
    {:noreply, %{state | refs: Map.put(state.refs, ref, id), ids: Map.put(state.ids, id, ref)}}
  end

  def handle_cast({:reported, id}, state) do
    case Map.pop(state.ids, id) do
      {nil, _ids} ->
        {:noreply, state}

      {ref, ids} ->
        Process.demonitor(ref, [:flush])
        {:noreply, %{state | refs: Map.delete(state.refs, ref), ids: ids}}
    end
  end

  @impl true
  def handle_info({:DOWN, ref, :process, _pid, reason}, state) do
    {id, refs} = Map.pop(state.refs, ref)
    ids = if id, do: Map.delete(state.ids, id), else: state.ids

    if id do
      case ActionLog.finish_job(
             id,
             :killed,
             "killed: " <> inspect(reason, limit: 25, printable_limit: 2_000)
           ) do
        {:notify, outbox} -> Notifier.publish(outbox)
        :already_final -> :ok
      end
    end

    {:noreply, %{state | refs: refs, ids: ids}}
  end
end
