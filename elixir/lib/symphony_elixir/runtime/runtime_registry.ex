defmodule SymphonyElixir.Runtime.RuntimeRegistry do
  @moduledoc """
  Registry of connected runtime workers, held on the control plane.

  A runtime worker (a `Runtime.WorkerClient` running on another host) opens a
  channel to the control plane and registers here, advertising the address the
  engine wire can reach its per-run room-servers at, the labels it carries, and
  its capacity. `Runtime.Placement` selects a worker from here when a run falls
  back to the `:remote` placement.

  Entries live in a `:public` ETS table keyed by `worker_id`, so a selection is
  a direct read off the runtime path with no GenServer round-trip. The owning
  GenServer monitors each worker's channel process and drops its entry when that
  process goes down, so a disconnected or crashed worker is never selected.
  """

  use GenServer

  require Logger

  @table :symphony_runtime_workers

  @typedoc """
  A registered runtime worker. `pid` is the worker's channel process on the
  control plane (monitored for liveness); `address` is the host the worker
  binds its per-run room-servers to, which the engine wire targets.
  """
  @type worker :: %{
          worker_id: String.t(),
          pid: pid(),
          address: String.t(),
          labels: [String.t()],
          capacity: non_neg_integer(),
          registered_at: integer()
        }

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    :ets.new(@table, [:named_table, :public, :set, read_concurrency: true])
    {:ok, %{monitors: %{}}}
  end

  @doc """
  Register (or refresh) a worker. The given `pid` (the worker's channel
  process) is monitored; when it goes down the entry is dropped. Re-registering
  the same `worker_id` replaces the prior entry.
  """
  @spec register(%{
          required(:worker_id) => String.t(),
          required(:pid) => pid(),
          required(:address) => String.t(),
          optional(:labels) => [String.t()],
          optional(:capacity) => non_neg_integer()
        }) :: :ok
  def register(%{worker_id: worker_id, pid: pid, address: address} = worker)
      when is_binary(worker_id) and is_pid(pid) and is_binary(address) do
    GenServer.call(__MODULE__, {:register, normalize(worker)})
  end

  @doc "Drop a worker by id. Idempotent."
  @spec unregister(String.t()) :: :ok
  def unregister(worker_id) when is_binary(worker_id) do
    GenServer.call(__MODULE__, {:unregister, worker_id})
  end

  @doc "Look up a worker by id, or `:error` if it is not (or no longer) registered."
  @spec get(String.t()) :: {:ok, worker()} | :error
  def get(worker_id) when is_binary(worker_id) do
    case :ets.whereis(@table) do
      :undefined ->
        :error

      _tid ->
        case :ets.lookup(@table, worker_id) do
          [{^worker_id, worker}] -> {:ok, worker}
          [] -> :error
        end
    end
  end

  @doc "All currently registered workers."
  @spec list() :: [worker()]
  def list do
    case :ets.whereis(@table) do
      :undefined -> []
      _tid -> :ets.tab2list(@table) |> Enum.map(fn {_id, worker} -> worker end)
    end
  end

  @doc """
  Select a healthy worker, restricted to those carrying `label` when one is
  given. Returns `{:ok, worker}` or `{:error, :no_worker}`. Picks the
  earliest-registered match for a stable choice; capacity-aware scheduling is a
  later refinement for multi-worker pools.
  """
  @spec select(String.t() | nil) :: {:ok, worker()} | {:error, :no_worker}
  def select(label \\ nil) do
    list()
    |> Enum.filter(fn worker -> is_nil(label) or label in worker.labels end)
    |> case do
      [] -> {:error, :no_worker}
      workers -> {:ok, Enum.min_by(workers, & &1.registered_at)}
    end
  end

  @impl true
  def handle_call({:register, worker}, _from, state) do
    ref = Process.monitor(worker.pid)
    :ets.insert(@table, {worker.worker_id, worker})

    # Drop any stale monitor for a prior connection of the same worker id.
    state = drop_monitor_for(state, worker.worker_id)
    Logger.info("RuntimeRegistry: registered worker=#{worker.worker_id} address=#{worker.address} labels=#{inspect(worker.labels)}")
    {:reply, :ok, put_in(state.monitors[ref], worker.worker_id)}
  end

  def handle_call({:unregister, worker_id}, _from, state) do
    :ets.delete(@table, worker_id)
    Logger.info("RuntimeRegistry: unregistered worker=#{worker_id}")
    {:reply, :ok, drop_monitor_for(state, worker_id)}
  end

  @impl true
  def handle_info({:DOWN, ref, :process, _pid, _reason}, state) do
    case Map.pop(state.monitors, ref) do
      {nil, _monitors} ->
        {:noreply, state}

      {worker_id, monitors} ->
        :ets.delete(@table, worker_id)
        Logger.info("RuntimeRegistry: worker=#{worker_id} disconnected; dropped")
        {:noreply, %{state | monitors: monitors}}
    end
  end

  defp drop_monitor_for(state, worker_id) do
    monitors =
      state.monitors
      |> Enum.reject(fn {ref, id} ->
        if id == worker_id do
          Process.demonitor(ref, [:flush])
          true
        else
          false
        end
      end)
      |> Map.new()

    %{state | monitors: monitors}
  end

  defp normalize(worker) do
    %{
      worker_id: worker.worker_id,
      pid: worker.pid,
      address: worker.address,
      labels: Map.get(worker, :labels, []),
      capacity: Map.get(worker, :capacity, 0),
      registered_at: System.monotonic_time(:millisecond)
    }
  end
end
