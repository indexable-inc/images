defmodule IxMcp.Application do
  @moduledoc """
  Supervision tree:

      IxMcp.Supervisor (one_for_one)
      ├── IxMcp.ActionLog       SQLite: sessions/topics/actions rows for every tools/call
      ├── IxMcp.Session         this instance's session/topic ids + display labels
      ├── IxMcp.Checkpoint      ETS keeper for workspace state (survives Workspace restarts)
      ├── IxMcp.Workspace       the shared binding + Macro.Env every cell sees
      ├── IxMcp.Jobs.Registry   id -> job process
      ├── IxMcp.Serve.State     served-app bookkeeping (url, jobs, gate outcome)
      ├── IxMcp.MCP.Notifier    server-initiated notification fan-out (+ outbox replay)
      ├── IxMcp.MCP.ClientRequests  server-initiated requests (elicitation) awaiting client replies
      ├── IxMcp.Jobs.Reaper     monitors job processes; finalizes any that die unreported
      ├── IxMcp.Jobs.Supervisor (DynamicSupervisor)  one child per cell/job
      │   └── IxMcp.Jobs.Job*   runs one evaluation in a monitored process
      ├── IxMcp.PrWatch.Supervisor (Task.Supervisor)  one task per PR watch
      ├── IxMcp.IssueWatch      (stdio + IX_MCP_ISSUE_WATCH_OWNERS set) new-issue channel feed
      ├── IxMcp.SessionWatch    (only when IX_MCP_STDIO=1) heartbeat + message/request-feed delivery
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
    # Before the tree: no cell exists yet, so the OS cwd is still the
    # directory the MCP client launched us in. Cmd pins every pathless
    # command to this capture -- a cell's File.cd!/1 moves the BEAM-global
    # cwd, and with several agents on one kernel that redirected a sibling
    # session's pathless git into a foreign worktree (#3902).
    IxMcp.Cmd.capture_launch_cwd()
    route_crash_dumps()
    install_crash_log()

    children =
      [
        # ActionLog before Session: Session lazily creates its rows through it.
        IxMcp.ActionLog,
        IxMcp.Session,
        IxMcp.Checkpoint,
        IxMcp.Workspace,
        {Registry, keys: :unique, name: IxMcp.Jobs.Registry},
        # Serve bookkeeping outlives the jobs it describes (gate results are
        # read after a serve's jobs die), so it lives here, not in a job.
        IxMcp.Serve.State,
        # Notifier and Reaper before the job supervisor: a job registers with
        # the reaper and publishes through the notifier, so both must be up
        # before any job can start (#3839).
        IxMcp.MCP.Notifier,
        IxMcp.MCP.ClientRequests,
        IxMcp.Jobs.Reaper,
        {DynamicSupervisor, name: IxMcp.Jobs.Supervisor, strategy: :one_for_one},
        {Task.Supervisor, name: IxMcp.PrWatch.Supervisor},
        # The depth-1 subagent surface (index#3700): harness first, then the
        # ledger that drains its lead mailbox.
        {AgentHarness, name: IxMcp.Agents.Harness, runner: IxMcp.Agents.CliRunner},
        {IxMcp.Agents.Events, harness: IxMcp.Agents.Harness}
      ] ++ issue_watch() ++ transport()

    # max_restarts above the default 3-in-5s (#3874): ActionLog's callers
    # now retry across its restarts, so a persistent fault there (disk
    # full, I/O error -- transient SQLITE_BUSY no longer crashes at all)
    # gets re-triggered quickly by the retries. The extra headroom keeps a
    # single faulting child from taking the whole kernel -- and every
    # running job -- down before the retries give up; a child that still
    # cannot hold up its part after ten restarts is a real fault and the
    # loud whole-app death stays.
    Supervisor.start_link(children,
      strategy: :one_for_one,
      max_restarts: 10,
      name: IxMcp.Supervisor
    )
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

  # index#3839: a persistent crash log next to the action log. The incident
  # left no SASL report anywhere -- the config :logger handler writes to
  # stderr, which the MCP client discards. A file handler (rotated, a few
  # MiB) means the next kernel-internal crash leaves an on-disk record of
  # every process that died, state and all. Adding it must never break boot,
  # so a failure here is swallowed -- a tool server must not die over its own
  # log. The in-memory test database has no directory to write into and opts
  # out.
  defp install_crash_log do
    db = IxMcp.ActionLog.db_path()

    if db != ":memory:" do
      dir = Path.dirname(db)
      File.mkdir_p!(dir)

      _ =
        :logger.add_handler(:ix_mcp_crash_log, :logger_std_h, %{
          level: :error,
          config: %{
            type: :file,
            file: String.to_charlist(Path.join(dir, "kernel.log")),
            max_no_bytes: 4_000_000,
            max_no_files: 3
          },
          formatter:
            Logger.Formatter.new(
              format: "$dateT$time $metadata[$level] $message\n",
              metadata: [:pid, :mfa, :crash_reason]
            )
        })
    end

    :ok
  rescue
    _ -> :ok
  end

  # Deferred (#3839): watch-job re-arm on kernel start. `Jobs.run(code, watch:
  # true)` already records the `watch` flag and the code on the durable jobs
  # row, so a future kernel could, at boot, re-arm its own unfinished watch
  # jobs from their recorded code and stamp every other unfinished job
  # `killed: kernel restart`. It is deferred deliberately: several server
  # instances share one actions.db, so a blanket startup sweep would clobber
  # a live sibling's running jobs (the same reason ActionLog does no startup
  # sweep of leftover `running` action rows). A safe implementation must
  # scope strictly to this instance's own jobs (by session), which needs a
  # durable instance identity that outlives a restart -- out of scope here.
  # The issue feed announces into a user-facing transport and polls GitHub,
  # so it rides the same flag as the transport: `mix test` and IEx sessions
  # get neither (#3877). The session watch (#3881) rides it too: its
  # heartbeat would otherwise register every test run in the directory and
  # its delivery loop would announce into a transport that is not there.
  defp issue_watch do
    case System.get_env("IX_MCP_STDIO") do
      "1" -> [IxMcp.IssueWatch, IxMcp.SessionWatch]
      _ -> []
    end
  end

  defp transport do
    case System.get_env("IX_MCP_STDIO") do
      "1" -> [IxMcp.MCP.Stdio]
      _ -> []
    end
  end
end
