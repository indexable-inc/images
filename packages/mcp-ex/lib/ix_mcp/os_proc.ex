defmodule IxMcp.OsProc do
  @moduledoc """
  Finds and kills the OS processes a job's cells spawned, so cancelling a job
  never leaks orphan subprocesses.

  There is no blessed shell helper in this server -- a cell that needs a
  subprocess writes `System.cmd/3` or opens a `Port` itself -- but the
  cancellation contract survives the cut: every port opened by any process in
  the job's process tree (identified by the job's group leader, which spawned
  processes inherit) is resolved to its OS pid, and that pid's whole descendant
  tree is killed.
  """

  @doc "BEAM processes whose group leader is `group_leader` (the job's tree)."
  @spec job_processes(pid()) :: [pid()]
  def job_processes(group_leader) do
    for pid <- Process.list(),
        pid != group_leader,
        {:group_leader, gl} <- [Process.info(pid, :group_leader)],
        gl == group_leader,
        do: pid
  end

  @doc "OS pids of ports connected to any process in the job's tree."
  @spec os_pids(pid()) :: [pos_integer()]
  def os_pids(group_leader) do
    owners = MapSet.new(job_processes(group_leader))

    for port <- Port.list(),
        info = Port.info(port),
        info != nil,
        MapSet.member?(owners, Keyword.get(info, :connected)),
        os_pid = Keyword.get(info, :os_pid),
        is_integer(os_pid),
        do: os_pid
  end

  @doc "Kill an OS pid and all of its descendants (TERM has no grace: jobs being cancelled are already condemned)."
  @spec kill_tree(pos_integer()) :: :ok
  def kill_tree(os_pid) do
    os_pid
    |> descendants()
    |> Enum.each(fn pid ->
      System.cmd("kill", ["-9", Integer.to_string(pid)], stderr_to_stdout: true)
    end)

    :ok
  end

  @doc "The pid plus its full descendant tree, children before parents are killed last."
  @spec descendants(pos_integer()) :: [pos_integer()]
  def descendants(os_pid) do
    children =
      case System.cmd("pgrep", ["-P", Integer.to_string(os_pid)], stderr_to_stdout: true) do
        {out, 0} ->
          out
          |> String.split("\n", trim: true)
          |> Enum.flat_map(fn line ->
            case Integer.parse(String.trim(line)) do
              {pid, ""} -> [pid]
              _ -> []
            end
          end)

        _ ->
          []
      end

    Enum.flat_map(children, &descendants/1) ++ [os_pid]
  end
end
