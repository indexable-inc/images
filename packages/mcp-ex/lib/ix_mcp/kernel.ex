defmodule IxMcp.Kernel do
  @moduledoc """
  The operations that needed signal hacks in the Python kernel, as ordinary
  BEAM introspection:

  * `trace/0` -- a live stack dump of every job's evaluation process (and the
    processes it spawned), taken with `Process.info/2` from outside. It works
    no matter what any cell is doing, because no cell can block this process.
  * `restart/0` -- kill every running job (including its OS subprocesses),
    restart the workspace, and restore bindings from the checkpoint table.
    Blast radius: this server's jobs, nothing else.
  """

  alias IxMcp.Jobs
  alias IxMcp.Jobs.Job

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
  @spec restart() :: %{jobs_cancelled: [String.t()], bindings_restored: non_neg_integer()}
  def restart do
    cancelled =
      for summary <- Jobs.running() do
        Jobs.cancel(summary.id)
        summary.id
      end

    :ok = Supervisor.terminate_child(IxMcp.Supervisor, IxMcp.Workspace)
    {:ok, _pid} = Supervisor.restart_child(IxMcp.Supervisor, IxMcp.Workspace)

    %{jobs_cancelled: cancelled, bindings_restored: length(IxMcp.Workspace.names())}
  end

  defp describe(pid, label) do
    case Process.info(pid, [:current_stacktrace, :status, :message_queue_len, :reductions]) do
      nil ->
        "#{label} #{inspect(pid)}: dead"

      info ->
        stack =
          info[:current_stacktrace]
          |> Enum.map_join("\n", fn entry -> "    " <> Exception.format_stacktrace_entry(entry) end)

        "#{label} #{inspect(pid)} [#{info[:status]}, queue=#{info[:message_queue_len]}, reductions=#{info[:reductions]}]\n" <>
          stack
    end
  end
end
