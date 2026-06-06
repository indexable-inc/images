defmodule SymphonyElixir.Application do
  @moduledoc """
  OTP application entrypoint.

  Boot order (one_for_one):

      Phoenix.PubSub         in-process eventbus
      Task.Supervisor        short-lived tasks (codex turns, trigger work)
      Config                 boot-time env snapshot
      GithubApp              mints and caches GitHub App installation tokens
      Catalog                watches skills/*.md, hot-reloads
      WorkflowCatalog        watches workflows/*.sym, hot-reloads the DSL ingress index
      CronState              persists per-workflow last_fired_at for cron workflows
      Runtime.Registry       name registry for per-run runtimes
      Runtime.Placement      per-run room-server placement registry (ixvm/host)
      Runtime.Supervisor     DynamicSupervisor for runs
      Triggers.Slack         polls Slack for completed huddles (opt-in)
      Triggers.Cron          fires cron-triggered workflows on a wall-clock cadence
      Endpoint               Phoenix HTTP + LiveView; also receives Linear webhooks
  """

  use Application

  @impl true
  def start(_type, _args) do
    if Application.get_env(:symphony_elixir, :auto_start, true) do
      start_supervised()
    else
      Supervisor.start_link([], strategy: :one_for_one, name: SymphonyElixir.Supervisor)
    end
  end

  defp start_supervised do
    :ok = SymphonyElixir.LogFile.configure()

    role = role()
    children = children_for(role)

    with {:ok, pid} <- Supervisor.start_link(children, strategy: :one_for_one, name: SymphonyElixir.Supervisor) do
      if role == :control_plane, do: SymphonyElixir.Runtime.Supervisor.resume_pending()
      {:ok, pid}
    end
  end

  # Read directly from the env, not the Config snapshot: the role decides
  # whether Config itself (and the rest of the tree) boots.
  defp role do
    case System.get_env("SYMPHONY_ROLE") do
      "worker" -> :worker
      _ -> :control_plane
    end
  end

  # The full control plane: triggers, webhooks, the run engine, the placement
  # registry, and the runtime-worker registry that backs :remote placement.
  defp children_for(:control_plane) do
    [
      {Phoenix.PubSub, name: SymphonyElixir.PubSub},
      {Task.Supervisor, name: SymphonyElixir.TaskSupervisor},
      SymphonyElixir.Config,
      SymphonyElixir.GithubApp,
      SymphonyElixir.Catalog,
      SymphonyElixir.WorkflowCatalog,
      SymphonyElixir.CronState,
      {Registry, keys: :unique, name: SymphonyElixir.Runtime.Registry},
      SymphonyElixir.Runtime.Placement,
      SymphonyElixir.Runtime.RuntimeRegistry,
      SymphonyElixir.Runtime.Supervisor,
      SymphonyElixir.Triggers.Slack,
      SymphonyElixir.Triggers.Cron,
      SymphonyElixirWeb.Endpoint
    ]
  end

  # A runtime worker: just enough to dial the control plane and provision
  # per-run room-servers on this host. No DB, triggers, engine, or HTTP surface.
  defp children_for(:worker) do
    [
      {Task.Supervisor, name: SymphonyElixir.TaskSupervisor},
      SymphonyElixir.Config,
      SymphonyElixir.Runtime.WorkerClient
    ]
  end
end
