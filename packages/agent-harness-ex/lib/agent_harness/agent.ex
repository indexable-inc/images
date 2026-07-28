defmodule AgentHarness.Agent do
  @moduledoc """
  One agent as a process: mailbox, status, budget, and the runner task.

  The GenServer only routes; the model conversation runs in a
  `Task.Supervisor` task (`async_nolink`) so message delivery, status
  queries, and cancellation stay live while the runner samples. The card's
  status ladder maps onto process state: a live task is `:working`, no task
  is `:idle`, and a dead process is `:terminated` (remembered by the roster
  in `AgentHarness.Coordinator`, since a dead process answers nothing).

  Delivery semantics live in `accept/2`, in priority order: a blocked
  `wait_for_message` caller is answered immediately, an idle subagent is
  woken with the message text as its new instructions, and otherwise the
  message queues until the runner's next `checkpoint/1` (which the runner
  calls after each tool result).
  """

  use GenServer, restart: :temporary

  alias AgentHarness.Context
  alias AgentHarness.Message
  alias AgentHarness.Names

  @type status :: :working | :idle

  @type start_args :: %{
          required(:harness) => atom(),
          required(:id) => String.t(),
          required(:role) => :lead | :subagent,
          required(:instructions) => String.t() | nil,
          required(:runner) => module(),
          required(:model) => term(),
          required(:token_budget) => pos_integer(),
          required(:opts) => keyword()
        }

  # -- client (called by the AgentHarness facade after a Registry lookup) --

  @spec start_link(start_args()) :: GenServer.on_start()
  def start_link(%{harness: harness, id: id} = args) do
    GenServer.start_link(__MODULE__, args, name: via(harness, id))
  end

  @spec via(atom(), String.t()) :: {:via, Registry, {atom(), {:agent, String.t()}}}
  def via(harness, id), do: {:via, Registry, {Names.registry(harness), {:agent, id}}}

  @spec deliver(GenServer.server(), Message.t()) :: :ok
  def deliver(server, %Message{} = msg), do: GenServer.call(server, {:deliver, msg})

  @doc "Blocks until a message arrives; the server owns the timeout."
  @spec wait_for_message(GenServer.server(), timeout()) :: {:ok, [Message.t()]} | :timeout
  def wait_for_message(server, timeout), do: GenServer.call(server, {:wait, timeout}, :infinity)

  @doc "Drain queued messages; the runner calls this after every tool result."
  @spec checkpoint(GenServer.server()) ::
          %{messages: [Message.t()], tokens_remaining: non_neg_integer()}
  def checkpoint(server), do: GenServer.call(server, :checkpoint)

  @spec add_usage(GenServer.server(), non_neg_integer()) ::
          {:ok, non_neg_integer()} | {:error, :budget_exhausted}
  def add_usage(server, tokens) when is_integer(tokens) and tokens >= 0 do
    GenServer.call(server, {:add_usage, tokens})
  end

  @spec status(GenServer.server()) :: status()
  def status(server), do: GenServer.call(server, :status)

  # -- server --

  @impl true
  def init(args) do
    # Trapping exits makes terminate/2 run on supervisor shutdown, so the
    # runner task dies with its agent instead of leaking past delete.
    Process.flag(:trap_exit, true)

    state = %{
      harness: args.harness,
      id: args.id,
      role: args.role,
      runner: args.runner,
      model: args.model,
      opts: args.opts,
      budget_limit: args.token_budget,
      budget_used: 0,
      status: :idle,
      task: nil,
      mailbox: :queue.new(),
      waiters: []
    }

    case args.role do
      :lead -> {:ok, state}
      :subagent -> {:ok, start_run(state, args.instructions)}
    end
  end

  @impl true
  def handle_call({:deliver, msg}, _from, state) do
    {:reply, :ok, accept(state, msg)}
  end

  def handle_call({:wait, timeout}, from, state) do
    case :queue.to_list(state.mailbox) do
      [] ->
        timer = schedule_wait_timeout(from, timeout)
        {:noreply, %{state | waiters: state.waiters ++ [{from, timer}]}}

      msgs ->
        {:reply, {:ok, msgs}, %{state | mailbox: :queue.new()}}
    end
  end

  def handle_call(:checkpoint, _from, state) do
    reply = %{
      messages: :queue.to_list(state.mailbox),
      tokens_remaining: tokens_remaining(state)
    }

    {:reply, reply, %{state | mailbox: :queue.new()}}
  end

  def handle_call({:add_usage, tokens}, _from, state) do
    next = %{state | budget_used: state.budget_used + tokens}

    reply =
      if next.budget_used > next.budget_limit do
        {:error, :budget_exhausted}
      else
        {:ok, tokens_remaining(next)}
      end

    {:reply, reply, next}
  end

  def handle_call(:status, _from, state) do
    {:reply, state.status, state}
  end

  @impl true
  def handle_cast({:deliver, msg}, state) do
    {:noreply, accept(state, msg)}
  end

  @impl true
  def handle_info({ref, result}, %{task: %{ref: ref}} = state) do
    Process.demonitor(ref, [:flush])
    {:noreply, finish_run(state, result)}
  end

  def handle_info({:DOWN, ref, :process, _pid, reason}, %{task: %{ref: ref}} = state) do
    {:noreply, finish_run(state, {:error, {:runner_crash, reason}})}
  end

  def handle_info({:wait_timeout, from}, state) do
    case Enum.split_with(state.waiters, fn {waiter, _timer} -> waiter == from end) do
      {[], _all} ->
        {:noreply, state}

      {[_expired], rest} ->
        GenServer.reply(from, :timeout)
        {:noreply, %{state | waiters: rest}}
    end
  end

  # Late task replies (after a timeout-triggered demonitor race) and stray
  # exits from trapped links have no state to change.
  def handle_info(_other, state), do: {:noreply, state}

  @impl true
  def terminate(_reason, %{task: %{pid: pid}} = state) do
    Task.Supervisor.terminate_child(Names.task_supervisor(state.harness), pid)
    :ok
  end

  def terminate(_reason, _state), do: :ok

  # -- delivery, in priority order (see @moduledoc) --

  defp accept(%{waiters: [{from, timer} | rest]} = state, msg) do
    cancel_timer(timer)
    GenServer.reply(from, {:ok, [msg]})
    %{state | waiters: rest}
  end

  defp accept(%{role: :subagent, status: :idle} = state, msg) do
    # The card: a finished subagent "idles until the lead wakes it with new
    # instructions". Waking is just messaging an idle agent; the message
    # text becomes the new instructions and starts a fresh run.
    start_run(state, msg.text)
  end

  defp accept(state, msg) do
    %{state | mailbox: :queue.in(msg, state.mailbox)}
  end

  defp start_run(state, instructions) do
    ctx = %Context{
      harness: state.harness,
      agent_id: state.id,
      model: state.model,
      token_budget: state.budget_limit,
      opts: state.opts
    }

    runner = state.runner

    task =
      Task.Supervisor.async_nolink(Names.task_supervisor(state.harness), fn ->
        runner.run(instructions, ctx)
      end)

    %{state | task: %{pid: task.pid, ref: task.ref}, status: :working}
  end

  defp finish_run(state, result) do
    route_to_lead(state, final_message(state, result))
    state = drop_waiters(%{state | task: nil, status: :idle})
    wake_from_mailbox(state)
  end

  # The only process that can legitimately be blocked in wait_for_message
  # here is the run that just ended (a crashed runner's stranded call), so
  # answer :timeout (a no-op for a dead caller) instead of leaving a stale
  # waiter to swallow the next wake message.
  defp drop_waiters(state) do
    Enum.each(state.waiters, fn {from, timer} ->
      cancel_timer(timer)
      GenServer.reply(from, :timeout)
    end)

    %{state | waiters: []}
  end

  # A message that landed during the final-composition window (after the
  # runner's last checkpoint, before its return) queued instead of waking
  # the agent. Same rule as the idle-wake clause in accept/2: the head
  # becomes the new instructions and the rest of the queue stays FIFO for
  # the next checkpoint.
  defp wake_from_mailbox(%{role: :subagent} = state) do
    case :queue.out(state.mailbox) do
      {{:value, msg}, rest} -> start_run(%{state | mailbox: rest}, msg.text)
      {:empty, _empty} -> state
    end
  end

  defp wake_from_mailbox(state), do: state

  defp final_message(state, {:ok, text}) when is_binary(text) do
    Message.new(state.id, Names.lead_id(), text, :final)
  end

  defp final_message(state, other) do
    Message.new(state.id, Names.lead_id(), inspect(other), :error)
  end

  defp route_to_lead(state, msg) do
    # Cast, not call: the lead may itself be mid-call into this agent, and a
    # final response must never deadlock the pair.
    case Registry.lookup(Names.registry(state.harness), {:agent, Names.lead_id()}) do
      [{pid, _value}] -> GenServer.cast(pid, {:deliver, msg})
      [] -> :ok
    end
  end

  defp tokens_remaining(state), do: max(state.budget_limit - state.budget_used, 0)

  defp schedule_wait_timeout(_from, :infinity), do: nil

  defp schedule_wait_timeout(from, timeout) do
    Process.send_after(self(), {:wait_timeout, from}, timeout)
  end

  defp cancel_timer(nil), do: :ok

  defp cancel_timer(timer) do
    Process.cancel_timer(timer)
    :ok
  end
end
