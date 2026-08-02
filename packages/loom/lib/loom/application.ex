defmodule Loom.Application do
  @moduledoc """
  Supervision: a `Registry` for agent ids and a `DynamicSupervisor` for
  agents. Agents are `:temporary` - a crashed agent is a failed run to
  report, not a process to blindly restart against a half-provisioned
  VM.
  """

  use Application

  @impl Application
  def start(_type, _args) do
    children = [
      Loom.Guard,
      {Registry, keys: :unique, name: Loom.Registry},
      {DynamicSupervisor, name: Loom.AgentSupervisor, strategy: :one_for_one}
    ]

    Supervisor.start_link(children, strategy: :one_for_one, name: Loom.Supervisor)
  end
end
