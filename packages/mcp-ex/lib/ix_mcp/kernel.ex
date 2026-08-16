defmodule IxMcp.Kernel do
  @moduledoc """
  The operations that needed signal hacks in the Python kernel, as ordinary
  BEAM introspection -- called from cells as `Ix.trace()` / `Ix.restart()`
  (the workspace prelude aliases this module as `Ix`; plain `Kernel` would
  shadow Elixir's). Because every cell is its own BEAM process, both work
  from a fresh cell even while other jobs run or wedge, which is why restart
  no longer needs to be an out-of-band MCP tool (#3532):

  * `trace/0` -- a live stack dump of every job's evaluation process (and the
    processes it spawned), taken with `Process.info/2` from outside. It works
    no matter what any cell is doing, because no cell can block this process.
  * `restart/0` -- kill every running job (including its OS subprocesses),
    restart the workspace, and restore bindings from the checkpoint table.
    Blast radius: this server's jobs, nothing else. When called from a cell,
    that cell's own job is spared so the restart runs to completion and the
    report comes back.
  * `bindings/0` -- every bound name with the cell that bound it. One
    kernel's workspace is shared by every agent riding its connection, so
    "who bound this?" is a real question (#3967).
  """

  alias IxMcp.Jobs
  alias IxMcp.Jobs.Job

  @doc """
  Every bound name with its owner: the job that wrote it, that job's intent,
  the value's shape, and when. The answer to a variable holding somebody
  else's work. Scoped to the calling cell's own workspace;
  `Ix.bindings("name")` asks about another one.
  """
  @spec bindings(String.t() | nil) :: [map()]
  def bindings(workspace \\ nil), do: IxMcp.Workspace.owners(workspace)

  @doc "Human-readable stack report for all running jobs and core server processes."
  @spec trace() :: String.t()
  def trace do
    job_sections =
      for summary <- Jobs.running() do
        {:ok, pid} = Jobs.lookup(summary.id)
        {eval_pid, io_proxy} = Job.procs(pid)

        spawned =
          io_proxy
          |> IxMcp.OsProc.job_processes()
          |> List.delete(eval_pid)

        section =
          [
            "job #{summary.id} (#{summary.intent || "no intent"}), " <>
              "running #{Float.round(summary.elapsed_s, 1)}s:",
            describe(eval_pid, "  eval")
          ] ++ Enum.map(spawned, &describe(&1, "  spawned"))

        Enum.join(section, "\n")
      end

    core_sections =
      for name <- [IxMcp.Workspace, IxMcp.Session, IxMcp.Checkpoint, IxMcp.MCP.Stdio],
          pid = Process.whereis(name),
          pid != nil,
          do: describe(pid, inspect(name))

    sections = job_sections ++ ["core processes:" | core_sections]

    case job_sections do
      [] -> Enum.join(["no running jobs" | sections], "\n")
      _ -> Enum.join(sections, "\n\n")
    end
  end

  @doc """
  Restart the evaluator: cancel all running jobs (their OS process trees die
  with them), restart the workspace process, and restore bindings from the
  checkpoint. Returns what was killed and what came back.
  """
  @spec restart() :: %{
          jobs_cancelled: [String.t()],
          bindings_restored: non_neg_integer(),
          workspaces_restored: %{String.t() => non_neg_integer()}
        }
  def restart do
    caller_gl = Process.group_leader()

    cancelled =
      for summary <- Jobs.running(), not caller_job?(summary.id, caller_gl) do
        Jobs.cancel(summary.id)
        summary.id
      end

    # The restart itself is only messages to processes that are not jobs
    # (the supervisor does the terminate/restart), so it completes even
    # though it runs inside a job's own process.
    :ok = Supervisor.terminate_child(IxMcp.Supervisor, IxMcp.Workspace)
    {:ok, _pid} = Supervisor.restart_child(IxMcp.Supervisor, IxMcp.Workspace)

    # Named workspaces restart the same way: capture the roster, bounce each
    # process, and let ensure/1 restore it from its own checkpoint row.
    named = IxMcp.Workspace.named()

    workspaces =
      for name <- named, into: %{} do
        with [{pid, _}] <- Registry.lookup(IxMcp.Workspaces.Registry, name) do
          _ = DynamicSupervisor.terminate_child(IxMcp.Workspaces.Supervisor, pid)
        end

        _pid = IxMcp.Workspace.ensure(name)
        {name, length(IxMcp.Workspace.names(name))}
      end

    %{
      jobs_cancelled: cancelled,
      bindings_restored: length(IxMcp.Workspace.names(IxMcp.Workspace.main())),
      workspaces_restored: workspaces
    }
  end

  # A cell calling `Ix.restart()` is itself a running job; cancelling it
  # would kill the restart mid-flight and eat the report. Every process a
  # cell spawns inherits the job's IOProxy as its group leader, so the
  # group leader identifies the requesting job from anywhere inside it.
  defp caller_job?(id, group_leader) do
    case Jobs.lookup(id) do
      {:ok, pid} -> match?({_eval, ^group_leader}, Job.procs(pid))
      {:error, :not_found} -> false
    end
  end

  defp describe(pid, label) do
    case Process.info(pid, [:current_stacktrace, :status, :message_queue_len, :reductions]) do
      nil ->
        "#{label} #{inspect(pid)}: dead"

      info ->
        stack =
          info[:current_stacktrace]
          |> Enum.map_join("\n", fn entry ->
            "    " <> Exception.format_stacktrace_entry(entry)
          end)

        "#{label} #{inspect(pid)} [#{info[:status]}, queue=#{info[:message_queue_len]}, reductions=#{info[:reductions]}]\n" <>
          stack
    end
  end
end
