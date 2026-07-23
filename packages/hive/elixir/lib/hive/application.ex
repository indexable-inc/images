defmodule Hive.Application do
  @moduledoc false

  use Application

  @impl true
  @spec start(Application.start_type(), term()) :: Supervisor.on_start()
  def start(_type, _args) do
    children = [
      # Shared id -> pid table. Lookups read ETS directly and run concurrently;
      # only registration (on agent start) goes through the registry process.
      {Registry, keys: :unique, name: Hive.Registry},
      # Spawns agents on demand at runtime, each supervised independently:
      # one crashing does not touch the others (:one_for_one).
      {DynamicSupervisor, strategy: :one_for_one, name: Hive.Swarm}
    ]

    opts = [strategy: :one_for_one, name: Hive.Supervisor]
    Supervisor.start_link(children, opts)
  end
end
