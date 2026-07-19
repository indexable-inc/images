defmodule AgentHarness.Names do
  @moduledoc false

  # One harness instance = one process family. Every registered name is
  # derived from the instance name the host chose, so several harnesses can
  # coexist in one VM (e.g. one per kernel session).

  @spec registry(atom()) :: atom()
  def registry(harness), do: Module.concat(harness, Registry)

  @spec agent_supervisor(atom()) :: atom()
  def agent_supervisor(harness), do: Module.concat(harness, AgentSupervisor)

  @spec task_supervisor(atom()) :: atom()
  def task_supervisor(harness), do: Module.concat(harness, TaskSupervisor)

  @spec coordinator(atom()) :: atom()
  def coordinator(harness), do: Module.concat(harness, Coordinator)

  @spec lead_id() :: String.t()
  def lead_id, do: "lead"
end
