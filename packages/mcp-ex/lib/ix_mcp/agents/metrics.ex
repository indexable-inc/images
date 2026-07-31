defmodule IxMcp.Agents.Metrics do
  @moduledoc """
  What each child agent is costing right now: its CLI process, every process
  that CLI spawned, and the per-agent totals.

  Identity is derived, not sampled. `IxMcp.Agents.Control` holds each running
  child's OS pid and `IxMcp.OsProc.descendants/1` walks that pid's children the
  same way cancellation does, so the tree measured here is the tree a cancel
  would kill. That is what makes a number attributable to an agent rather than
  merely true of the host.

  Three honesty rules, because a plausible wrong number here is worse than a
  gap:

    * `cpu_pct` is what `ps` reports, which is the process's average over its
      whole lifetime rather than an instantaneous rate. A long-lived child that
      is spiking right now still reads low. A rate would need two samples a
      known interval apart, which this call deliberately is not.
    * memory is unavailable on darwin. macOS 27's `ps` answers `ps: rss:
      requires entitlement` and then DROPS that column while still printing the
      others, so asking for it would silently shift every field one place
      (which is exactly how this was found). Columns are chosen per platform
      and `rss_kb` reports `:unavailable` there. `%mem` and `vsz` are entitled
      too, and deriving bytes from a percentage rounded to 0.1% of host RAM
      would be a guess wearing a number's clothes. ENG#4487 tracks the
      `proc_pid_rusage` NIF, whose `phys_footprint` is the right darwin figure
      anyway.
    * per-pid IO exists only on Linux (`/proc/<pid>/io`), so darwin reports
      `:unavailable` rather than a zero: a fabricated zero is indistinguishable
      from a genuinely idle process.
  """

  alias IxMcp.Agents.Control
  alias IxMcp.Cmd
  alias IxMcp.OsProc

  @typedoc "A measurement the platform would not give us."
  @type unavailable :: :unavailable

  @typedoc "One OS process in an agent's tree."
  @type proc :: %{
          os_pid: pos_integer(),
          ppid: pos_integer(),
          cpu_pct: float(),
          rss_kb: non_neg_integer() | unavailable(),
          command: String.t(),
          io: io()
        }

  @typedoc "Bytes this process moved, or nothing we can honestly report."
  @type io :: %{read_bytes: non_neg_integer(), write_bytes: non_neg_integer()} | unavailable()

  @typedoc "One agent's tree plus its totals."
  @type agent :: %{
          id: String.t(),
          backend: atom(),
          os_pid: pos_integer() | nil,
          procs: [proc()],
          cpu_pct: float(),
          rss_kb: non_neg_integer() | unavailable()
        }

  @doc """
  Every agent with a live runner, its OS process tree, and its totals.

  A pid that exited between the walk and the sample is simply absent from its
  agent's `procs`. Measured on macOS 27 and Linux: `ps` given a mix of live and
  dead pids prints the survivors and exits 0, so ordinary turnover is not an
  error at any level. (It refuses the whole request only for a pid above the
  system ceiling, which cannot come from a walk of the live process table.)
  """
  @spec tree() :: [agent()]
  def tree do
    entries = Control.all()
    trees = Map.new(entries, fn {id, entry} -> {id, tree_pids(entry)} end)

    samples =
      trees
      |> Enum.flat_map(fn {_id, pids} -> pids end)
      |> Enum.uniq()
      |> sample()

    entries
    |> Enum.map(fn {id, entry} -> agent_row(id, entry, Map.get(trees, id, []), samples) end)
    |> Enum.sort_by(& &1.id)
  end

  @doc "Sample the given OS pids in one `ps` pass; a pid that is gone is absent."
  @spec sample([pos_integer()]) :: %{pos_integer() => proc()}
  def sample([]), do: %{}

  def sample(os_pids) do
    {format, shape} = columns()
    args = ["-o", format, "-p", Enum.join(os_pids, ",")]

    # Status is deliberately ignored: the rows are the answer. A dead pid in the
    # list costs nothing (ps prints the survivors and still exits 0), and on
    # darwin the status is 1 for every call anyway because the denied memory
    # column is reported as a keyword error.
    {out, _status} = Cmd.run("ps", args)

    out
    |> String.split("\n", trim: true)
    |> Enum.flat_map(&parse_row(&1, shape))
    |> Map.new(fn proc -> {proc.os_pid, proc} end)
  end

  # See the moduledoc: asking darwin for rss shifts every field, so ask only
  # for what the platform grants.
  defp columns do
    case :os.type() do
      {:unix, :linux} -> {"pid=,ppid=,pcpu=,rss=,comm=", :with_rss}
      _darwin_or_other -> {"pid=,ppid=,pcpu=,comm=", :without_rss}
    end
  end

  defp tree_pids(%{os_pid: nil}), do: []
  defp tree_pids(%{os_pid: os_pid}), do: OsProc.descendants(os_pid)

  defp agent_row(id, entry, pids, samples) do
    procs = Enum.flat_map(pids, fn pid -> List.wrap(Map.get(samples, pid)) end)

    %{
      id: id,
      backend: entry.backend,
      os_pid: entry.os_pid,
      procs: procs,
      cpu_pct: procs |> Enum.map(& &1.cpu_pct) |> Enum.sum() |> Float.round(1),
      rss_kb: total_rss(procs)
    }
  end

  # One unmeasurable process makes the total unmeasurable: summing the rest
  # would report a subset as if it were the whole tree.
  defp total_rss(procs) do
    if Enum.any?(procs, &(&1.rss_kb == :unavailable)) do
      :unavailable
    else
      procs |> Enum.map(& &1.rss_kb) |> Enum.sum()
    end
  end

  defp parse_row(line, :with_rss) do
    case String.split(String.trim(line), ~r/\s+/, parts: 5) do
      [pid, ppid, cpu, rss, command] -> row(pid, ppid, cpu, rss, command)
      _short -> []
    end
  end

  defp parse_row(line, :without_rss) do
    case String.split(String.trim(line), ~r/\s+/, parts: 4) do
      [pid, ppid, cpu, command] -> row(pid, ppid, cpu, :unavailable, command)
      _short -> []
    end
  end

  defp row(pid, ppid, cpu, rss, command) do
    with {os_pid, ""} <- Integer.parse(pid),
         {parent, ""} <- Integer.parse(ppid),
         {cpu_pct, _rest} <- Float.parse(cpu),
         {:ok, rss_kb} <- rss_kb(rss) do
      [
        %{
          os_pid: os_pid,
          ppid: parent,
          cpu_pct: cpu_pct,
          rss_kb: rss_kb,
          command: command,
          io: io(os_pid)
        }
      ]
    else
      _unparsable -> []
    end
  end

  defp rss_kb(:unavailable), do: {:ok, :unavailable}

  defp rss_kb(text) do
    case Integer.parse(text) do
      {kb, ""} -> {:ok, kb}
      _junk -> :error
    end
  end

  defp io(os_pid) do
    case :os.type() do
      {:unix, :linux} -> proc_io(os_pid)
      _other -> :unavailable
    end
  end

  defp proc_io(os_pid) do
    case File.read("/proc/#{os_pid}/io") do
      {:ok, body} ->
        fields = Map.new(Regex.scan(~r/^(\w+):\s+(\d+)$/m, body), fn [_all, k, v] -> {k, v} end)
        %{read_bytes: to_int(fields["read_bytes"]), write_bytes: to_int(fields["write_bytes"])}

      # A pid that exited, or one whose /proc entry we may not read.
      {:error, _reason} ->
        :unavailable
    end
  end

  defp to_int(nil), do: 0
  defp to_int(value), do: String.to_integer(value)
end
