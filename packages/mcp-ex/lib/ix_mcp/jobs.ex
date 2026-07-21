defmodule IxMcp.Jobs do
  @moduledoc """
  The job registry facade -- available in every cell (the workspace prelude
  aliases it), so job control needs no extra tools, exactly like the Python
  kernel's `jobs` dict:

      Jobs.tail("ab12", 20)     # last lines of a run's output
      Jobs.await("ab12")        # block this cell (only this cell) until done
      Jobs.cancel("ab12")       # kill it, and every OS process it spawned
      Jobs.history()            # recent runs, newest first

  Runs live in a durable ledger (`IxMcp.ActionLog`), not just in memory
  (#3839): `tail/head/grep/output/history/get` fall back to the SQLite tables
  when the job process is gone, so a crashed or killed run is still readable
  and still on record. `history/1` is a view over the `jobs` table scoped to
  this server instance's session.
  """

  alias IxMcp.ActionLog
  alias IxMcp.Jobs.Job
  alias IxMcp.Session

  @typedoc "A recorded run, as `history/1` returns it."
  @type entry :: %{
          id: String.t(),
          intent: String.t() | nil,
          session: String.t() | nil,
          topic: String.t() | nil,
          code: String.t(),
          status: Job.status(),
          started_at: DateTime.t() | nil,
          elapsed_s: float() | nil
        }

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
        {Job, {id, code, Keyword.take(opts, [:intent, :action_id, :watch])}}
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

  @doc "Summary of a job -- live from its process, or reconstructed from the ledger."
  @spec get(String.t()) :: Job.summary()
  def get(id) do
    live_or_ledger(id, &Job.summary/1, &summary_from_ledger/1)
  end

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

  @spec cancel(String.t()) :: :ok | {:error, :finished | Job.status()}
  def cancel(id) do
    case lookup(id) do
      {:ok, pid} ->
        try do
          Job.cancel(pid)
        catch
          # The registry unregisters dead pids asynchronously, so a lookup
          # can briefly return a process that no longer exists; that
          # :noproc means the same thing as a missed lookup (#3538).
          :exit, {:noproc, _call} -> report_dead(id)
        end

      {:error, :not_found} ->
        report_dead(id)
    end
  end

  # A job whose process is gone cannot be cancelled, but the ledger still
  # holds its terminal state (#3839): report it. Raise only for ids this
  # server never ran -- #3538 showed that raising "no such job" about an id
  # `history/1` still listed sent the operator chasing a phantom.
  defp report_dead(id) do
    case ActionLog.job(id) do
      %{status: status} -> {:error, status}
      nil -> raise ArgumentError, "no such job: #{inspect(id)}"
    end
  end

  @spec result(String.t()) :: {:ok, term()} | {:error, :running | String.t()}
  def result(id) do
    live_or_ledger(id, &Job.result/1, &result_from_ledger/1)
  end

  # The result term dies with the process; the ledger keeps only its rendered
  # form, so a gone job answers with that string, never a live term.
  defp result_from_ledger(id) do
    case ActionLog.job(id) do
      nil -> raise ArgumentError, "no such job: #{inspect(id)}"
      %{status: :running} -> {:error, :running}
      %{result: result} -> {:error, result || "job no longer resident"}
    end
  end

  @doc "Full captured output -- from the live buffer, or the durable table if the job is gone."
  @spec output(String.t()) :: String.t()
  def output(id) do
    live_or_ledger(id, &Job.output/1, &output_from_ledger/1)
  end

  defp output_from_ledger(id) do
    case ActionLog.job(id) do
      nil -> raise ArgumentError, "no such job: #{inspect(id)}"
      _job -> ActionLog.job_output(id)
    end
  end

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

  @doc "Recent runs of this server instance, newest first."
  @spec history(pos_integer()) :: [entry()]
  def history(n \\ 20) do
    %{session_id: session_id} = Session.ids()

    session_id
    |> ActionLog.recent_jobs(n)
    |> Enum.map(&to_entry/1)
  end

  defp lines(id) do
    id |> output() |> String.split("\n")
  end

  defp with_job(id, fun) do
    case lookup(id) do
      {:ok, pid} -> fun.(pid)
      {:error, :not_found} -> raise ArgumentError, "no such job: #{inspect(id)}"
    end
  end

  # Read from the live process, falling back to the durable ledger when it is
  # gone -- including the registered-but-dead-pid window: the Registry
  # unregisters dead pids asynchronously (#3538), so a lookup can hand back a
  # corpse whose GenServer.call exits :noproc. That means the same as a
  # missing process (#3839).
  defp live_or_ledger(id, live_fun, dead_fun) do
    case lookup(id) do
      {:ok, pid} ->
        try do
          live_fun.(pid)
        catch
          :exit, {reason, _call} when reason in [:noproc, :normal] -> dead_fun.(id)
        end

      {:error, :not_found} ->
        dead_fun.(id)
    end
  end

  defp summary_from_ledger(id) do
    case ActionLog.job(id) do
      nil ->
        raise ArgumentError, "no such job: #{inspect(id)}"

      job ->
        %{
          id: job.id,
          status: job.status,
          running: job.status == :running,
          intent: job.intent,
          elapsed_s: (job.elapsed_ms || 0) / 1000,
          output_bytes: job.output_bytes,
          diagnostics: [],
          result: job.result
        }
    end
  end

  defp to_entry(job) do
    %{
      id: job.id,
      intent: job.intent,
      session: job.session,
      topic: job.topic,
      code: job.code,
      status: job.status,
      started_at: parse_dt(job.started_at),
      elapsed_s: if(job.elapsed_ms, do: job.elapsed_ms / 1000)
    }
  end

  defp parse_dt(nil), do: nil

  defp parse_dt(iso) do
    case DateTime.from_iso8601(iso) do
      {:ok, dt, _} -> dt
      _ -> nil
    end
  end

  defp generate_id do
    Base.encode16(:crypto.strong_rand_bytes(4), case: :lower)
  end
end
