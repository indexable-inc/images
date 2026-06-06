defmodule SymphonyElixir.Runtime.Supervisor do
  @moduledoc """
  Dynamic supervisor for the per-run `Runtime` GenServers. Each active
  `RunGraph` runs as one child here, so a crashing run kills only that
  run, not the orchestrator.

  The supervisor owns two operations the orchestrator needs:

  - `start_run/2` schedules a fresh `RunGraph` (already materialized by
    `IR.Materializer`) under supervision.
  - `resume_pending/1` reloads non-terminal runs from `IR.Store` on boot
    and restarts each with `recover: true`, so the runtime reconciles
    orphaned `:running` nodes (the BEAM-restart half of #90) before
    resuming. A run already live is left alone.

  The engine client is injected, defaulting to `Runtime.RoomEngineClient`
  for production. Tests pass a fake to avoid a live room-server.
  """

  use DynamicSupervisor
  require Logger

  alias SymphonyElixir.IR.{RunGraph, Store}
  alias SymphonyElixir.Runtime
  alias SymphonyElixir.Runtime.Placement

  @default_engine SymphonyElixir.Runtime.RoomEngineClient

  @spec start_link(keyword()) :: Supervisor.on_start()
  def start_link(opts \\ []) do
    DynamicSupervisor.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    DynamicSupervisor.init(strategy: :one_for_one)
  end

  @doc """
  Start a run under supervision. `opts` are forwarded to `Runtime`
  (`:engine`, `:store_opts`, `:recover`); `:engine` defaults to the
  production room-server client.
  """
  @spec start_run(RunGraph.t(), keyword()) :: DynamicSupervisor.on_start_child()
  def start_run(%RunGraph{} = graph, opts \\ []) do
    DynamicSupervisor.start_child(__MODULE__, {Runtime, {graph, with_default_engine(opts)}})
  end

  @doc """
  Reload every non-terminal run from `IR.Store` and resume it with
  `recover: true`. Idempotent: a run with a live runtime (already
  registered) is skipped. Called once at boot.
  """
  @spec resume_pending(keyword()) :: :ok
  def resume_pending(opts \\ []) do
    store_opts = Keyword.get(opts, :store_opts, [])
    graphs = Store.load_all(store_opts)

    # Reap room-server units orphaned by a prior restart and re-attach the
    # ones whose run we are about to resume, before resuming. The placement
    # registry is in-memory, so without this a resumed run would collide on
    # its deterministic unit name and every terminal run's pre-restart unit
    # would linger. Share the loaded set so the store is read once.
    placement(opts).reconcile(graphs, opts)

    graphs
    |> Enum.filter(&resumable?/1)
    |> Enum.each(fn graph -> resume_one(graph, opts) end)
  end

  defp resume_one(%RunGraph{} = graph, opts) do
    resume_opts = opts |> Keyword.put(:recover, true) |> with_default_engine()

    case start_run(graph, resume_opts) do
      {:ok, _pid} -> :ok
      {:error, {:already_started, _pid}} -> :ok
      {:error, reason} -> Logger.warning("Failed to resume IR run #{graph.run_id}: #{inspect(reason)}")
    end
  end

  defp resumable?(%RunGraph{status: status}), do: status in [:pending, :running]

  defp with_default_engine(opts), do: Keyword.put_new(opts, :engine, @default_engine)

  # The placement module, overridable in tests with a stub so resume can be
  # exercised without a real systemd host. Defaults to the real registry.
  defp placement(opts), do: Keyword.get(opts, :placement, Placement)
end
