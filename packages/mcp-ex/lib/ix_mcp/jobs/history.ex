defmodule IxMcp.Jobs.History do
  @moduledoc """
  Ordered record of every run: id, intent, session/topic at start time, a code
  preview, and final status. This is what `Jobs.history/1` pages and what the
  `exec` feed groups by session and topic.

  A GenServer rather than an Agent so it can monitor each recording job
  process: a job that dies without reporting -- a crash, a kill -- still gets
  its row driven to a terminal status by the DOWN handler below. Before
  #3538 a crashed job GenServer left its row saying `:running` forever about
  a process that no longer existed, with nothing left alive to correct it.
  """

  use GenServer

  @type entry :: %{
          id: String.t(),
          intent: String.t() | nil,
          session: String.t() | nil,
          topic: String.t() | nil,
          code: String.t(),
          status: IxMcp.Jobs.Job.status(),
          started_at: DateTime.t(),
          elapsed_s: float() | nil
        }

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @doc "Record a new run for the calling job process."
  @spec record(map()) :: :ok
  def record(entry) do
    GenServer.call(__MODULE__, {:record, entry})
  end

  @spec finished(String.t(), IxMcp.Jobs.Job.status(), float()) :: :ok
  def finished(id, status, elapsed_s) do
    GenServer.call(__MODULE__, {:finished, id, status, elapsed_s})
  end

  @doc "Latest `n` runs, newest first."
  @spec list(pos_integer()) :: [entry()]
  def list(n \\ 20) do
    GenServer.call(__MODULE__, {:list, n})
  end

  @doc "The recorded run with this id, or nil."
  @spec get(String.t()) :: entry() | nil
  def get(id) do
    GenServer.call(__MODULE__, {:get, id})
  end

  @impl true
  def init(_) do
    {:ok, %{entries: [], monitors: %{}}}
  end

  @impl true
  def handle_call({:record, entry}, {job, _tag}, state) do
    # Monitor the recording job: if its process dies before `finished/3`,
    # the DOWN clause below is the only thing left that can finalize the
    # row (#3538 -- a poisoned output buffer crashed the job GenServer
    # between eval success and finish, orphaning the row at :running).
    ref = Process.monitor(job)
    entry = Map.put(entry, :elapsed_s, nil)

    {:reply, :ok,
     %{
       state
       | entries: [entry | state.entries],
         monitors: Map.put(state.monitors, ref, entry.id)
     }}
  end

  def handle_call({:finished, id, status, elapsed_s}, _from, state) do
    entries =
      Enum.map(state.entries, fn
        %{id: ^id} = entry -> %{entry | status: status, elapsed_s: elapsed_s}
        entry -> entry
      end)

    {:reply, :ok, %{state | entries: entries, monitors: demonitor(state.monitors, id)}}
  end

  def handle_call({:list, n}, _from, state) do
    {:reply, Enum.take(state.entries, n), state}
  end

  def handle_call({:get, id}, _from, state) do
    {:reply, Enum.find(state.entries, &(&1.id == id)), state}
  end

  @impl true
  def handle_info({:DOWN, ref, :process, _pid, _reason}, state) do
    {id, monitors} = Map.pop(state.monitors, ref)

    # A dead job that never reported a terminal status failed, whatever the
    # exit reason: the row must stop claiming :running about a process that
    # no longer exists (#3538). The :running guard keeps rows that finished
    # or were cancelled exactly as they reported themselves.
    entries =
      Enum.map(state.entries, fn
        %{id: ^id, status: :running} = entry ->
          %{
            entry
            | status: :failed,
              elapsed_s: DateTime.diff(DateTime.utc_now(), entry.started_at, :millisecond) / 1000
          }

        entry ->
          entry
      end)

    {:noreply, %{state | entries: entries, monitors: monitors}}
  end

  # The row is terminal; the monitor has nothing left to guard. Flush it so
  # the eventual (normal) death of the long-lived job process costs nothing.
  defp demonitor(monitors, id) do
    case Enum.find(monitors, fn {_ref, job_id} -> job_id == id end) do
      {ref, _job_id} ->
        Process.demonitor(ref, [:flush])
        Map.delete(monitors, ref)

      nil ->
        monitors
    end
  end
end
