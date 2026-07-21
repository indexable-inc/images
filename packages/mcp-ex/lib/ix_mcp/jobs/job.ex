defmodule IxMcp.Jobs.Job do
  @moduledoc """
  One cell run. The GenServer is the job's control point; the evaluation
  itself happens in a separate spawned process so that a blocking cell blocks
  only itself -- the scheduler preempts it, other jobs and the server keep
  running, and cancellation is `Process.exit(pid, :kill)` plus OS
  subprocess-tree cleanup rather than a signal hack against a wedged loop.

  Processes compute; data remembers (#3839). Every terminal transition
  (`done`/`failed`/`cancelled`/`killed`) is one atomic write to the durable
  ledger (`IxMcp.ActionLog`): the `jobs` row and its `actions` row go terminal
  together and a notification lands in the outbox, so no death is silent and
  the two logs can never disagree. Output streams into the `job_output` table
  as it is produced, so `Jobs.tail/output/result` keep working after the
  process is gone. A hot ETS buffer stays for fast live reads.

  The control process is deliberately survivable. It traps exits and only
  *links* its IOProxy, so an IOProxy crash -- the shared-component failure
  that in #3839 killed every linked job brutally, skipping finish and
  notification -- arrives as a trappable EXIT and becomes a recorded `killed`
  transition with the output captured so far. A hard external kill of the
  control process itself (which trapping cannot catch) is caught by the
  single reaper (`IxMcp.Jobs.Reaper`), which writes the `killed` transition
  from outside.
  """

  use GenServer, restart: :temporary

  alias IxMcp.ActionLog
  alias IxMcp.Evaluator
  alias IxMcp.Evaluator.IOProxy
  alias IxMcp.Jobs.Reaper
  alias IxMcp.MCP.Notifier
  alias IxMcp.OsProc
  alias IxMcp.Session
  alias IxMcp.Workspace

  # How often a running exec's eval process is stack-sampled into the action
  # log (Process.info(pid, :current_stacktrace) -- external, works on a
  # wedged process). Tests shrink it via app env to keep the suite fast.
  @stack_sample_interval_ms Application.compile_env(:ix_mcp, :stack_sample_interval_ms, 1000)

  # How often buffered output is flushed from the hot ETS buffer to the
  # durable `job_output` table. A hard kill loses at most this window of the
  # very latest output (already-flushed output survives, which is the point).
  @flush_interval_ms Application.compile_env(:ix_mcp, :output_flush_interval_ms, 250)

  # Per-job output cap. Beyond it, output is dropped (head kept) and the drop
  # is recorded, so one runaway cell cannot fill the ledger. 8 MiB is far
  # above any legitimate cell's output.
  @output_cap 8 * 1024 * 1024

  @enforce_keys [:id, :code]
  defstruct [
    :id,
    :code,
    :intent,
    :action_id,
    :session_id,
    :session,
    :topic,
    :watch,
    :buffer,
    :counter,
    :io_proxy,
    :eval_pid,
    :eval_ref,
    :started_mono,
    :started_at,
    :finished_mono,
    :result,
    status: :running,
    diagnostics: [],
    subscribers: [],
    flushed_seq: nil,
    flushed_dropped: 0,
    flush_scheduled: false
  ]

  @type status :: :running | :done | :failed | :cancelled | :killed

  @type t :: %__MODULE__{
          id: String.t(),
          code: String.t(),
          intent: String.t() | nil,
          action_id: integer() | nil,
          session_id: integer() | nil,
          session: String.t() | nil,
          topic: String.t() | nil,
          watch: boolean(),
          buffer: :ets.tid() | nil,
          counter: :counters.counters_ref() | nil,
          io_proxy: pid() | nil,
          eval_pid: pid() | nil,
          eval_ref: reference() | nil,
          started_mono: integer() | nil,
          started_at: DateTime.t() | nil,
          finished_mono: integer() | nil,
          result: {:value, term()} | {:failure, String.t()} | nil,
          status: status(),
          diagnostics: [String.t()],
          subscribers: [pid()],
          flushed_seq: integer() | nil,
          flushed_dropped: non_neg_integer(),
          flush_scheduled: boolean()
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

  @doc "Full captured output of the job as one binary (from the hot buffer)."
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
    # Trapping exits is what makes the control process survivable: a linked
    # IOProxy crash arrives as {:EXIT, io_proxy, reason} instead of taking
    # this process down with it (#3839).
    Process.flag(:trap_exit, true)

    session = Session.get()
    %{session_id: session_id} = Session.ids()

    state = %__MODULE__{
      id: id,
      code: code,
      intent: Keyword.get(opts, :intent),
      action_id: Keyword.get(opts, :action_id),
      watch: Keyword.get(opts, :watch, false),
      session_id: session_id,
      session: session.name,
      topic: session.topic,
      buffer: :ets.new(:job_output, [:ordered_set, :public]),
      counter: :counters.new(2, [:write_concurrency]),
      started_mono: System.monotonic_time(:millisecond),
      started_at: DateTime.utc_now()
    }

    {:ok, state, {:continue, :spawn_eval}}
  end

  @impl true
  def handle_continue(:spawn_eval, state) do
    # Record the durable jobs row and arm the reaper before anything can die,
    # so a death in the first instants is still finalized (#3839).
    ActionLog.job_started(%{
      id: state.id,
      session_id: state.session_id,
      action_id: state.action_id,
      intent: state.intent,
      session_name: state.session,
      topic_name: state.topic,
      code: state.code,
      watch: state.watch,
      started_at: DateTime.to_iso8601(state.started_at)
    })

    Reaper.watch(state.id, self())

    buffer = state.buffer
    counter = state.counter
    sink = fn chunk -> capture(buffer, counter, chunk) end
    {:ok, io_proxy} = IOProxy.start_link(sink)

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

    if state.action_id, do: Process.send_after(self(), :sample_stack, @stack_sample_interval_ms)
    schedule_flush()

    {:noreply,
     %{state | io_proxy: io_proxy, eval_pid: eval_pid, eval_ref: eval_ref, flush_scheduled: true}}
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
    # linked process). The exit reason IS the crash report, state and all --
    # format it whole. This is the cell author's failure, hence `failed`
    # (distinct from `killed`, which is the machinery dying under the job).
    {:noreply, finish(state, :failed, {:failure, Exception.format_exit(reason)}, [])}
  end

  # The IOProxy (linked) crashed under a running job. Trapping exits turned
  # what #3839 made a brutal linked death into this trappable signal: record
  # a `killed` transition with the output captured so far, rather than
  # vanishing silently.
  def handle_info(
        {:EXIT, io_proxy, reason},
        %{io_proxy: io_proxy, status: :running} = state
      )
      when reason != :normal do
    Process.demonitor(state.eval_ref, [:flush])
    if is_pid(state.eval_pid), do: Process.exit(state.eval_pid, :kill)

    {:noreply,
     finish(state, :killed, {:failure, "killed: io proxy exited: " <> inspect(reason)}, [])}
  end

  # A supervisor shutdown (or a normal linked exit) is not a job death we
  # record, but we must still terminate: trapping exits means the signal no
  # longer stops us automatically, and ignoring it would stall whole-tree
  # shutdown until the supervisor's timeout brutally kills us.
  def handle_info({:EXIT, _pid, :normal}, state), do: {:stop, :normal, state}
  def handle_info({:EXIT, _pid, :shutdown}, state), do: {:stop, :shutdown, state}

  def handle_info({:EXIT, _pid, {:shutdown, _} = reason}, state),
    do: {:stop, reason, state}

  # Any other linked EXIT under a still-running job is machinery we don't
  # model dying; leave the job alone (the reaper covers a true control-process
  # death).
  def handle_info({:EXIT, _pid, _reason}, state), do: {:noreply, state}

  def handle_info(:flush, %{status: :running} = state) do
    state = %{flush(state) | flush_scheduled: true}
    schedule_flush()
    {:noreply, state}
  end

  def handle_info(:flush, state), do: {:noreply, %{state | flush_scheduled: false}}

  # Sampling stops itself once the run finished (no reschedule); the
  # status='running' guard in the log makes a racing last sample harmless.
  def handle_info(:sample_stack, %{status: :running} = state) do
    case Process.info(state.eval_pid, :current_stacktrace) do
      {:current_stacktrace, frames} ->
        shown =
          case Evaluator.prune_stacktrace(frames) do
            [] -> frames
            pruned -> pruned
          end

        stack = JSON.encode!(Enum.map(shown, &Exception.format_stacktrace_entry/1))
        ActionLog.update_stack(state.action_id, stack, cell_line(frames))

      nil ->
        :ok
    end

    Process.send_after(self(), :sample_stack, @stack_sample_interval_ms)
    {:noreply, state}
  end

  def handle_info(:sample_stack, state), do: {:noreply, state}

  def handle_info(_msg, state), do: {:noreply, state}

  # -- internals ---------------------------------------------------------------

  defp cell_line(frames) do
    Enum.find_value(frames, fn {_mod, _fun, _arity, meta} ->
      line = meta[:line]
      if meta[:file] == ~c"cell" and is_integer(line) and line > 0, do: line
    end)
  end

  defp finish(state, status, result, diags) do
    state = %{
      state
      | status: status,
        result: result,
        diagnostics: diags,
        finished_mono: System.monotonic_time(:millisecond)
    }

    # Persist all captured output before the transition, so a reader that
    # sees the job finish finds its output already complete on disk.
    state = flush(state)

    # The one atomic terminal transition: jobs row + actions row + outbox,
    # committed together (#3839). Whoever writes it first wins; the reaper's
    # racing `killed` attempt then no-ops.
    case ActionLog.finish_job(state.id, status, render_result(state)) do
      {:notify, outbox} -> Notifier.publish(outbox)
      :already_final -> :ok
    end

    Reaper.reported(state.id)

    summary = build_summary(state)
    Enum.each(state.subscribers, fn pid -> send(pid, {:ix_job_finished, state.id, summary}) end)
    %{state | subscribers: []}
  end

  # Capture one output chunk into the hot buffer, honoring the per-job cap.
  # Only binaries enter the buffer (IOProxy's convert/2 guarantees valid
  # UTF-8); the guard turns any regression into a failed put_chars at the
  # call site rather than a poisoned job record (#3538).
  defp capture(buffer, counter, chunk) when is_binary(chunk) do
    if :counters.get(counter, 1) < @output_cap do
      :ets.insert(buffer, {System.unique_integer([:monotonic]), chunk})
      :counters.add(counter, 1, byte_size(chunk))
    else
      :counters.add(counter, 2, byte_size(chunk))
    end
  end

  # Move buffer rows not yet persisted into the durable table, in one batch.
  defp flush(state) do
    after_seq = state.flushed_seq

    match =
      case after_seq do
        nil -> [{{:"$1", :"$2"}, [], [{{:"$1", :"$2"}}]}]
        seq -> [{{:"$1", :"$2"}, [{:>, :"$1", seq}], [{{:"$1", :"$2"}}]}]
      end

    case :ets.select(state.buffer, match) do
      [] ->
        maybe_flush_dropped(state)

      rows ->
        rows = Enum.sort(rows)
        dropped_now = :counters.get(state.counter, 2)
        ActionLog.append_job_output(state.id, rows, dropped_now - state.flushed_dropped)
        {last_seq, _} = List.last(rows)
        %{state | flushed_seq: last_seq, flushed_dropped: dropped_now}
    end
  end

  # No new rows, but the drop counter may have advanced (output over the cap
  # is counted, not buffered) -- record the delta so truncation is durable.
  defp maybe_flush_dropped(state) do
    dropped_now = :counters.get(state.counter, 2)

    if dropped_now > state.flushed_dropped do
      ActionLog.append_job_output(state.id, [], dropped_now - state.flushed_dropped)
      %{state | flushed_dropped: dropped_now}
    else
      state
    end
  end

  defp schedule_flush, do: Process.send_after(self(), :flush, @flush_interval_ms)

  defp build_summary(state) do
    finished = state.finished_mono || System.monotonic_time(:millisecond)

    %{
      id: state.id,
      status: state.status,
      running: state.status == :running,
      intent: state.intent,
      elapsed_s: (finished - state.started_mono) / 1000,
      output_bytes: :counters.get(state.counter, 1),
      diagnostics: state.diagnostics,
      result: render_result(state)
    }
  end

  defp render_result(%{result: {:value, value}}), do: Evaluator.render(value)
  defp render_result(%{result: {:failure, message}}), do: message
  defp render_result(_), do: nil
end
