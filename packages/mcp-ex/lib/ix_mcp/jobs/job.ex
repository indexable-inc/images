defmodule IxMcp.Jobs.Job do
  @moduledoc """
  One cell run. The GenServer is the job's record and control point; the
  evaluation itself happens in a separate spawned process so that a blocking
  cell blocks only itself -- the scheduler preempts it, other jobs and the
  server keep running, and cancellation is `Process.exit(pid, :kill)` plus OS
  subprocess-tree cleanup rather than a signal hack against a wedged loop.

  The process stays alive after the run finishes: it holds the output buffer
  (an ETS table) and the result term, so paging (`tail/head/grep/...`) and
  `Jobs.result/1` work for as long as the server lives, like the Python
  `jobs` dict.
  """

  use GenServer, restart: :temporary

  alias IxMcp.Evaluator
  alias IxMcp.Evaluator.IOProxy
  alias IxMcp.Jobs.History
  alias IxMcp.MCP.Notifier
  alias IxMcp.OsProc
  alias IxMcp.Session
  alias IxMcp.Workspace

  @enforce_keys [:id, :code]
  defstruct [
    :id,
    :code,
    :intent,
    :session,
    :topic,
    :buffer,
    :io_proxy,
    :eval_pid,
    :eval_ref,
    :started_mono,
    :started_at,
    :finished_mono,
    :result,
    status: :running,
    diagnostics: [],
    subscribers: []
  ]

  @type status :: :running | :done | :failed | :cancelled

  @type t :: %__MODULE__{
          id: String.t(),
          code: String.t(),
          intent: String.t() | nil,
          session: String.t() | nil,
          topic: String.t() | nil,
          buffer: :ets.tid() | nil,
          io_proxy: pid() | nil,
          eval_pid: pid() | nil,
          eval_ref: reference() | nil,
          started_mono: integer() | nil,
          started_at: DateTime.t() | nil,
          finished_mono: integer() | nil,
          result: {:value, term()} | {:failure, String.t()} | nil,
          status: status(),
          diagnostics: [String.t()],
          subscribers: [pid()]
        }

  @type summary :: %{
          id: String.t(),
          status: status(),
          running: boolean(),
          intent: String.t() | nil,
          elapsed_s: float(),
          output_bytes: non_neg_integer(),
          diagnostics: [String.t()],
          result: String.t() | nil
        }

  # -- client ----------------------------------------------------------------

  @spec start_link({String.t(), String.t(), keyword()}) :: GenServer.on_start()
  def start_link({id, code, opts}) do
    GenServer.start_link(__MODULE__, {id, code, opts}, name: via(id))
  end

  @spec via(String.t()) :: {:via, Registry, {IxMcp.Jobs.Registry, String.t()}}
  def via(id), do: {:via, Registry, {IxMcp.Jobs.Registry, id}}

  @spec summary(GenServer.server()) :: summary()
  def summary(server), do: GenServer.call(server, :summary)

  @doc "The job's result value. `{:error, :running}` while the job runs."
  @spec result(GenServer.server()) :: {:ok, term()} | {:error, :running | String.t()}
  def result(server), do: GenServer.call(server, :result)

  @spec cancel(GenServer.server()) :: :ok | {:error, :finished}
  def cancel(server), do: GenServer.call(server, :cancel)

  @doc "The evaluation process and IO proxy (for tracing from outside)."
  @spec procs(GenServer.server()) :: {pid(), pid()}
  def procs(server), do: GenServer.call(server, :procs)

  @doc """
  Wait until the job finishes, up to `timeout_ms`. Returns the final summary
  or `:timeout` -- in which case the job keeps running in the background,
  which is the entire budget-then-background contract.
  """
  @spec await(GenServer.server(), timeout()) :: {:ok, summary()} | :timeout
  def await(server, timeout_ms) do
    case GenServer.call(server, {:subscribe, self()}) do
      {:finished, summary} ->
        {:ok, summary}

      :subscribed ->
        receive do
          {:ix_job_finished, _id, summary} -> {:ok, summary}
        after
          timeout_ms ->
            GenServer.cast(server, {:unsubscribe, self()})

            # The finish notification may have raced the timeout; prefer it.
            receive do
              {:ix_job_finished, _id, summary} -> {:ok, summary}
            after
              0 -> :timeout
            end
        end
    end
  end

  @doc "Full captured output of the job as one binary."
  @spec output(GenServer.server()) :: String.t()
  def output(server) do
    server
    |> GenServer.call(:buffer)
    |> :ets.tab2list()
    |> Enum.sort()
    |> Enum.map_join("", fn {_seq, chunk} -> chunk end)
  end

  # -- server ----------------------------------------------------------------

  @impl true
  def init({id, code, opts}) do
    session = Session.get()

    state = %__MODULE__{
      id: id,
      code: code,
      intent: Keyword.get(opts, :intent),
      session: session.name,
      topic: session.topic,
      buffer: :ets.new(:job_output, [:ordered_set, :public]),
      started_mono: System.monotonic_time(:millisecond),
      started_at: DateTime.utc_now()
    }

    {:ok, state, {:continue, :spawn_eval}}
  end

  @impl true
  def handle_continue(:spawn_eval, state) do
    buffer = state.buffer
    {:ok, io_proxy} = IOProxy.start_link(fn chunk -> append(buffer, chunk) end)

    job = self()
    code = state.code

    {eval_pid, eval_ref} =
      spawn_monitor(fn ->
        Process.group_leader(self(), io_proxy)
        {binding, env} = Workspace.snapshot()

        outcome =
          case Evaluator.eval(code, binding, env) do
            {:ok, value, binding, env, diags} ->
              Workspace.merge(binding, env)
              {:done, value, diags}

            {:parse_error, message} ->
              {:failed, "parse error: " <> message, []}

            {:runtime_error, formatted, diags} ->
              {:failed, formatted, diags}
          end

        send(job, {:eval_finished, self(), outcome})
      end)

    History.record(initial_history(state))
    {:noreply, %{state | io_proxy: io_proxy, eval_pid: eval_pid, eval_ref: eval_ref}}
  end

  @impl true
  def handle_call(:summary, _from, state), do: {:reply, build_summary(state), state}

  def handle_call(:result, _from, %{status: :running} = state),
    do: {:reply, {:error, :running}, state}

  def handle_call(:result, _from, %{result: {:value, value}} = state),
    do: {:reply, {:ok, value}, state}

  def handle_call(:result, _from, %{result: {:failure, message}} = state),
    do: {:reply, {:error, message}, state}

  def handle_call(:cancel, _from, %{status: :running} = state) do
    # OS subprocesses first: once the BEAM processes die, the ports close and
    # the pids can no longer be discovered.
    state.io_proxy |> OsProc.os_pids() |> Enum.each(&OsProc.kill_tree/1)

    state.io_proxy
    |> OsProc.job_processes()
    |> Enum.each(fn pid -> Process.exit(pid, :kill) end)

    Process.demonitor(state.eval_ref, [:flush])
    {:reply, :ok, finish(state, :cancelled, {:failure, "cancelled"}, [])}
  end

  def handle_call(:cancel, _from, state), do: {:reply, {:error, :finished}, state}

  def handle_call({:subscribe, pid}, _from, %{status: :running} = state) do
    {:reply, :subscribed, %{state | subscribers: [pid | state.subscribers]}}
  end

  def handle_call({:subscribe, _pid}, _from, state) do
    {:reply, {:finished, build_summary(state)}, state}
  end

  def handle_call(:buffer, _from, state), do: {:reply, state.buffer, state}

  def handle_call(:procs, _from, state), do: {:reply, {state.eval_pid, state.io_proxy}, state}

  @impl true
  def handle_cast({:unsubscribe, pid}, state) do
    {:noreply, %{state | subscribers: List.delete(state.subscribers, pid)}}
  end

  @impl true
  def handle_info({:eval_finished, pid, outcome}, %{eval_pid: pid} = state) do
    Process.demonitor(state.eval_ref, [:flush])

    state =
      case outcome do
        {:done, value, diags} -> finish(state, :done, {:value, value}, diags)
        {:failed, message, diags} -> finish(state, :failed, {:failure, message}, diags)
      end

    {:noreply, state}
  end

  def handle_info(
        {:DOWN, ref, :process, _pid, reason},
        %{eval_ref: ref, status: :running} = state
      ) do
    # The cell's process died without reporting: a crash (exit, throw from a
    # linked process, kill). The exit reason IS the crash report, state and
    # all -- format it whole.
    {:noreply, finish(state, :failed, {:failure, Exception.format_exit(reason)}, [])}
  end

  def handle_info(_msg, state), do: {:noreply, state}

  # -- internals ---------------------------------------------------------------

  defp finish(state, status, result, diags) do
    state = %{
      state
      | status: status,
        result: result,
        diagnostics: diags,
        finished_mono: System.monotonic_time(:millisecond)
    }

    summary = build_summary(state)
    Enum.each(state.subscribers, fn pid -> send(pid, {:ix_job_finished, state.id, summary}) end)
    History.finished(state.id, status, summary.elapsed_s)
    Notifier.job_finished(summary)
    %{state | subscribers: []}
  end

  defp append(buffer, chunk) do
    :ets.insert(buffer, {System.unique_integer([:monotonic]), chunk})
  end

  defp build_summary(state) do
    finished = state.finished_mono || System.monotonic_time(:millisecond)

    %{
      id: state.id,
      status: state.status,
      running: state.status == :running,
      intent: state.intent,
      elapsed_s: (finished - state.started_mono) / 1000,
      output_bytes:
        :ets.foldl(fn {_seq, chunk}, acc -> acc + byte_size(chunk) end, 0, state.buffer),
      diagnostics: state.diagnostics,
      result: render_result(state)
    }
  end

  defp render_result(%{result: {:value, value}}), do: Evaluator.render(value)
  defp render_result(%{result: {:failure, message}}), do: message
  defp render_result(_), do: nil

  defp initial_history(state) do
    %{
      id: state.id,
      intent: state.intent,
      session: state.session,
      topic: state.topic,
      code: String.slice(state.code, 0, 200),
      status: :running,
      started_at: state.started_at
    }
  end
end
