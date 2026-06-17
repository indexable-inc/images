defmodule Hive do
  @moduledoc """
  A fully connected mesh of agent actors.

  Each agent is a `Hive.Agent` GenServer, spawned at runtime under a
  DynamicSupervisor (`Hive.Swarm`) and addressed by a logical id through
  `Hive.Registry`. There are no edges to wire: connectivity is implicit, because
  any agent can resolve any id to a live pid and message it.
  """

  @doc "Spawn a new agent named `id` into the mesh."
  @spec spawn_agent(Hive.Agent.id()) :: DynamicSupervisor.on_start_child()
  def spawn_agent(id) when is_atom(id) do
    DynamicSupervisor.start_child(Hive.Swarm, {Hive.Agent, id})
  end

  @doc "Spin up a few agents and have them talk to each other."
  @spec demo() :: :ok
  def demo do
    for id <- [:planner, :executor, :critic], do: spawn_agent(id)

    Hive.Agent.whisper(:executor, :planner, {:do, "step 1"})
    Hive.Agent.whisper(:critic, :executor, {:review, "result of step 1"})
    Hive.Agent.broadcast(:planner, :standup)

    # whispers are async casts; let them drain before we read inboxes
    Process.sleep(50)

    IO.puts("")

    for id <- Enum.sort(Hive.Agent.ids()) do
      IO.puts("#{id} inbox: #{inspect(Hive.Agent.inbox(id))}")
    end

    :ok
  end
end
