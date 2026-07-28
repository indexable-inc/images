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

  require Logger

  # Retry cadence for a `killed` transition the ledger could not take
  # (#3874). The reaper's monitor map is the only record of which jobs are
  # still guarded, so this process must never die over a ledger call: it
  # re-arms the write instead.
  @finalize_retries 10
  @finalize_retry_ms 1_000

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @doc """
  Monitor a job control process so its unreported death is finalized.
  `start` is the job's start metadata (`ActionLog.job_start()`): the
  finalizing transition hands it to `ActionLog.finish_job/5`, which
  reconstructs the jobs row when the `job_started` write itself was lost
  under load (#4082) -- without it, a kill after a lost start write
  no-opped as `:already_final` and the job vanished from the record.
  """
  @spec watch(String.t(), pid(), map() | nil) :: :ok
  def watch(id, pid, start \\ nil) do
    GenServer.cast(__MODULE__, {:watch, id, pid, start})
  end

  @doc "A job reported a terminal transition itself; stop guarding it."
  @spec reported(String.t()) :: :ok
  def reported(id) do
    GenServer.cast(__MODULE__, {:reported, id})
  end

  @impl true
  def init(_) do
    {:ok, %{refs: %{}, ids: %{}, starts: %{}}}
  end

  @impl true
  def handle_cast({:watch, id, pid, start}, state) do
    ref = Process.monitor(pid)

    {:noreply,
     %{
       state
       | refs: Map.put(state.refs, ref, id),
         ids: Map.put(state.ids, id, ref),
         starts: put_start(state.starts, id, start)
     }}
  end

  def handle_cast({:reported, id}, state) do
    case Map.pop(state.ids, id) do
      {nil, _ids} ->
        {:noreply, state}

      {ref, ids} ->
        Process.demonitor(ref, [:flush])

        {:noreply,
         %{
           state
           | refs: Map.delete(state.refs, ref),
             ids: ids,
             starts: Map.delete(state.starts, id)
         }}
    end
  end

  @impl true
  def handle_info({:DOWN, ref, :process, _pid, reason}, state) do
    {id, refs} = Map.pop(state.refs, ref)
    ids = if id, do: Map.delete(state.ids, id), else: state.ids
    {start, starts} = if id, do: Map.pop(state.starts, id), else: {nil, state.starts}

    if id do
      result = "killed: " <> inspect(reason, limit: 25, printable_limit: 2_000)
      finalize(id, result, start, @finalize_retries)
    end

    {:noreply, %{state | refs: refs, ids: ids, starts: starts}}
  end

  def handle_info({:finalize, id, result, start, attempts}, state) do
    finalize(id, result, start, attempts)
    {:noreply, state}
  end

  # Drive the ledger terminal for a job that died unreported. An exit from
  # the ledger call (it died mid-request, #3874) must not crash the reaper
  # -- that would drop every monitor it holds -- so the attempt re-arms
  # itself instead.
  defp finalize(id, result, start, attempts) do
    case ActionLog.finish_job(id, :killed, result, start: start) do
      {:notify, outbox} -> Notifier.publish(outbox)
      :already_final -> :ok
    end
  catch
    :exit, reason ->
      if attempts > 0 do
        Process.send_after(
          self(),
          {:finalize, id, result, start, attempts - 1},
          @finalize_retry_ms
        )
      else
        Logger.error("reaper: job #{id} terminal write lost: #{inspect(reason, limit: 5)}")
      end
  end

  defp put_start(starts, _id, nil), do: starts
  defp put_start(starts, id, start), do: Map.put(starts, id, start)
end
