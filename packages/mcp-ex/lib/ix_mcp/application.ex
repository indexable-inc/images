defmodule IxMcp.Application do
  @moduledoc """
  Supervision tree:

      IxMcp.Supervisor (one_for_one)
      ├── IxMcp.ActionLog       SQLite: sessions/topics/actions rows for every tools/call
      ├── IxMcp.Session         this instance's session/topic ids + display labels
      ├── IxMcp.Checkpoint      ETS keeper for workspace state (survives Workspace restarts)
      ├── IxMcp.Workspace       the shared binding + Macro.Env every cell sees
      ├── IxMcp.Jobs.Registry   id -> job process
      ├── IxMcp.Jobs.Supervisor (DynamicSupervisor)  one child per cell/job
      │   └── IxMcp.Jobs.Job*   runs one evaluation in a monitored process
      ├── IxMcp.Jobs.History    ordered record of every run
      ├── IxMcp.MCP.Notifier    server-initiated notification fan-out
      ├── IxMcp.PrWatch.Supervisor (Task.Supervisor)  one task per PR watch
      ├── IxMcp.Agents.Harness     (AgentHarness) depth-1 subagent processes (#3700)
      ├── IxMcp.Agents.Events      subagent ledger: events, finals, graph, notifications
      └── IxMcp.MCP.Stdio       (only when IX_MCP_STDIO=1) the stdio transport

  The transport is opt-in via environment so `mix test` and IEx sessions get
  the full evaluator without a reader loop competing for stdin. Before the
  tree starts, `ERL_CRASH_DUMP` is pointed into the action log's directory:
  a BEAM crash dump otherwise lands in the inherited cwd (#3539).
  """

  use Application

  @impl true
  def start(_type, _args) do
    route_crash_dumps()

    children =
      [
        # ActionLog before Session: Session lazily creates its rows through it.
        IxMcp.ActionLog,
        IxMcp.Session,
        IxMcp.Checkpoint,
        IxMcp.Workspace,
        {Registry, keys: :unique, name: IxMcp.Jobs.Registry},
        {DynamicSupervisor, name: IxMcp.Jobs.Supervisor, strategy: :one_for_one},
        IxMcp.Jobs.History,
        IxMcp.MCP.Notifier,
        {Task.Supervisor, name: IxMcp.PrWatch.Supervisor},
        # The depth-1 subagent surface (index#3700): harness first, then the
        # ledger that drains its lead mailbox.
        {AgentHarness, name: IxMcp.Agents.Harness, runner: IxMcp.Agents.CliRunner},
        {IxMcp.Agents.Events, harness: IxMcp.Agents.Harness}
      ] ++ transport()

    Supervisor.start_link(children, strategy: :one_for_one, name: IxMcp.Supervisor)
  end

  # index#3539: without ERL_CRASH_DUMP the BEAM writes erl_crash.dump into
  # whatever cwd it inherited from the MCP client -- the 2026-07-17 startup
  # crash dumped 3.6MB into ~/.config/nix, which then got committed by
  # accident. The runtime reads the variable at dump time, not at boot, so
  # exporting it before any child can fail routes every dump from
  # application start on into the state dir that already holds the action
  # log. An explicit operator ERL_CRASH_DUMP wins; the in-memory test
  # database has no directory to aim at, so it opts out.
  defp route_crash_dumps do
    db = IxMcp.ActionLog.db_path()

    if is_nil(System.get_env("ERL_CRASH_DUMP")) and db != ":memory:" do
      dir = Path.dirname(db)
      File.mkdir_p!(dir)
      System.put_env("ERL_CRASH_DUMP", Path.join(dir, "erl_crash.dump"))
    end

    :ok
  end

  defp transport do
    case System.get_env("IX_MCP_STDIO") do
      "1" -> [IxMcp.MCP.Stdio]
      _ -> []
    end
  end
end
