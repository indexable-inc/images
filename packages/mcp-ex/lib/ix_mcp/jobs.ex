defmodule IxMcp.Jobs do
  @moduledoc """
  The job registry facade -- available in every cell (the workspace prelude
  aliases it), so job control needs no extra tools, exactly like the Python
  kernel's `jobs` dict:

      Jobs.tail("ab12", 20)     # last lines of a run's output
      Jobs.await("ab12")        # block this cell (only this cell) until done
      Jobs.cancel("ab12")       # kill it, and every OS process it spawned
      Jobs.history()            # recent runs, newest first
  """

  alias IxMcp.Jobs.History
  alias IxMcp.Jobs.Job

  @doc """
  Start `code` as a new job and wait up to `budget_s` seconds. Finished or
  not, returns `{summary, output}` -- when still running, the job continues
  in the background under its returned id.
  """
  @spec run(String.t(), keyword()) :: {Job.summary(), String.t()}
  def run(code, opts \\ []) when is_binary(code) do
    budget_s = Keyword.get(opts, :budget, 15)
    id = generate_id()

    {:ok, pid} =
      DynamicSupervisor.start_child(
        IxMcp.Jobs.Supervisor,
        {Job, {id, code, Keyword.take(opts, [:intent, :action_id])}}
      )

    case Job.await(pid, round(budget_s * 1000)) do
      {:ok, summary} -> {summary, Job.output(pid)}
      :timeout -> {Job.summary(pid), Job.output(pid)}
    end
  end

  @doc "Look up a job process by id."
  @spec lookup(String.t()) :: {:ok, pid()} | {:error, :not_found}
  def lookup(id) do
    case Registry.lookup(IxMcp.Jobs.Registry, id) do
      [{pid, _}] -> {:ok, pid}
      [] -> {:error, :not_found}
    end
  end

  @spec get(String.t()) :: Job.summary()
  def get(id), do: with_job(id, &Job.summary/1)

  @doc "Block the calling process (a cell, never the server) until the job finishes."
  @spec await(String.t(), timeout()) :: Job.summary() | :timeout
  def await(id, timeout_ms \\ :infinity) do
    with_job(id, fn pid ->
      case Job.await(pid, timeout_ms) do
        {:ok, summary} -> summary
        :timeout -> :timeout
      end
    end)
  end

  @spec cancel(String.t()) :: :ok | {:error, :finished}
  def cancel(id), do: with_job(id, &Job.cancel/1)

  @spec result(String.t()) :: {:ok, term()} | {:error, :running | String.t()}
  def result(id), do: with_job(id, &Job.result/1)

  @spec output(String.t()) :: String.t()
  def output(id), do: with_job(id, &Job.output/1)

  @doc "Last `n` lines of the job's output."
  @spec tail(String.t(), pos_integer()) :: String.t()
  def tail(id, n \\ 20) do
    id |> lines() |> Enum.take(-n) |> Enum.join("\n")
  end

  @doc "First `n` lines of the job's output."
  @spec head(String.t(), pos_integer()) :: String.t()
  def head(id, n \\ 20) do
    id |> lines() |> Enum.take(n) |> Enum.join("\n")
  end

  @doc "Lines `first..last` (1-based, inclusive)."
  @spec lines(String.t(), pos_integer(), pos_integer()) :: String.t()
  def lines(id, first, last) when first >= 1 and last >= first do
    id |> lines() |> Enum.slice((first - 1)..(last - 1)) |> Enum.join("\n")
  end

  @doc "Lines `[from, to)` (0-based, exclusive), Python-slice style."
  @spec slice(String.t(), non_neg_integer(), non_neg_integer()) :: String.t()
  def slice(id, from, to) when from >= 0 and to >= from do
    id |> lines() |> Enum.slice(from, to - from) |> Enum.join("\n")
  end

  @doc "Output lines matching `pattern` (a regex or a plain substring)."
  @spec grep(String.t(), Regex.t() | String.t()) :: String.t()
  def grep(id, %Regex{} = pattern) do
    id |> lines() |> Enum.filter(&Regex.match?(pattern, &1)) |> Enum.join("\n")
  end

  def grep(id, pattern) when is_binary(pattern) do
    id |> lines() |> Enum.filter(&String.contains?(&1, pattern)) |> Enum.join("\n")
  end

  @doc "Ids and summaries of jobs that are still running."
  @spec running() :: [Job.summary()]
  def running do
    IxMcp.Jobs.Supervisor
    |> DynamicSupervisor.which_children()
    |> Enum.flat_map(fn
      {_, pid, :worker, _} when is_pid(pid) -> [Job.summary(pid)]
      _ -> []
    end)
    |> Enum.filter(& &1.running)
  end

  @doc "Recent runs, newest first."
  @spec history(pos_integer()) :: [History.entry()]
  def history(n \\ 20), do: History.list(n)

  defp lines(id) do
    id |> output() |> String.split("\n")
  end

  defp with_job(id, fun) do
    case lookup(id) do
      {:ok, pid} -> fun.(pid)
      {:error, :not_found} -> raise ArgumentError, "no such job: #{inspect(id)}"
    end
  end

  defp generate_id do
    Base.encode16(:crypto.strong_rand_bytes(4), case: :lower)
  end
end
