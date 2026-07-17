defmodule IxMcp.Application do
  @moduledoc """
  Supervision tree:

      IxMcp.Supervisor (one_for_one)
      ├── IxMcp.Session         session name / topic metadata
      ├── IxMcp.ActionLog       SQLite append-only record of every tools/call
      ├── IxMcp.Checkpoint      ETS keeper for workspace state (survives Workspace restarts)
      ├── IxMcp.Workspace       the shared binding + Macro.Env every cell sees
      ├── IxMcp.Jobs.Registry   id -> job process
      ├── IxMcp.Jobs.Supervisor (DynamicSupervisor)  one child per cell/job
      │   └── IxMcp.Jobs.Job*   runs one evaluation in a monitored process
      ├── IxMcp.Jobs.History    ordered record of every run
      ├── IxMcp.MCP.Notifier    server-initiated notification fan-out
      ├── IxMcp.PrWatch.Supervisor (Task.Supervisor)  one task per PR watch
      └── IxMcp.MCP.Stdio       (only when IX_MCP_STDIO=1) the stdio transport

  The transport is opt-in via environment so `mix test` and IEx sessions get
  the full evaluator without a reader loop competing for stdin.
  """

  use Application

  @impl true
  def start(_type, _args) do
    children =
      [
        IxMcp.Session,
        IxMcp.ActionLog,
        IxMcp.Checkpoint,
        IxMcp.Workspace,
        {Registry, keys: :unique, name: IxMcp.Jobs.Registry},
        {DynamicSupervisor, name: IxMcp.Jobs.Supervisor, strategy: :one_for_one},
        IxMcp.Jobs.History,
        IxMcp.MCP.Notifier,
        {Task.Supervisor, name: IxMcp.PrWatch.Supervisor}
      ] ++ transport()

    Supervisor.start_link(children, strategy: :one_for_one, name: IxMcp.Supervisor)
  end

  defp transport do
    case System.get_env("IX_MCP_STDIO") do
      "1" -> [IxMcp.MCP.Stdio]
      _ -> []
    end
  end
end
