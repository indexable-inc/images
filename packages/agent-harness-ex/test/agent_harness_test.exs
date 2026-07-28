defmodule AgentHarnessTest do
  use ExUnit.Case, async: true

  # Test runners: each simulates one shape of agentic loop and reports its
  # progress to the test process via the :notify pid in ctx.opts.

  defmodule SleepyRunner do
    @behaviour AgentHarness.Runner

    @impl true
    def run(instructions, ctx) do
      send(Keyword.fetch!(ctx.opts, :notify), {:started, self(), ctx.agent_id, instructions})

      receive do
        :finish -> {:ok, "finished"}
      end
    end
  end

  defmodule CheckpointRunner do
    @behaviour AgentHarness.Runner

    @impl true
    def run(_instructions, ctx) do
      notify = Keyword.fetch!(ctx.opts, :notify)
      send(notify, {:runner, self(), ctx.agent_id})
      loop(ctx, notify)
    end

    # Each :tool_result from the test stands in for one finished tool call;
    # the checkpoint after it is where queued messages become visible.
    defp loop(ctx, notify) do
      receive do
        :tool_result ->
          {:ok, %{messages: msgs}} = AgentHarness.checkpoint(ctx.harness, ctx.agent_id)
          send(notify, {:checkpoint, msgs})
          loop(ctx, notify)

        :finish ->
          {:ok, "done"}
      end
    end
  end

  defmodule WaitingRunner do
    @behaviour AgentHarness.Runner

    @impl true
    def run(_instructions, ctx) do
      notify = Keyword.fetch!(ctx.opts, :notify)
      send(notify, {:waiting, self(), ctx.agent_id})
      {:ok, msgs} = AgentHarness.wait_for_message(ctx.harness, ctx.agent_id)
      send(notify, {:got, msgs})
      {:ok, "woken"}
    end
  end

  defmodule EchoRunner do
    @behaviour AgentHarness.Runner

    @impl true
    def run(instructions, ctx) do
      send(Keyword.fetch!(ctx.opts, :notify), {:run, ctx.agent_id, instructions})
      {:ok, "answer:" <> instructions}
    end
  end

  defmodule LatchRunner do
    @behaviour AgentHarness.Runner

    # Holds the final response open behind a test-controlled latch: the
    # window between the runner's last checkpoint and its return is where a
    # send_message must not strand (the agent is still :working, so nothing
    # would wake it later).
    @impl true
    def run(instructions, ctx) do
      notify = Keyword.fetch!(ctx.opts, :notify)
      send(notify, {:run, self(), ctx.agent_id, instructions})
      loop(instructions, ctx, notify)
    end

    defp loop(instructions, ctx, notify) do
      receive do
        :checkpoint ->
          {:ok, %{messages: msgs}} = AgentHarness.checkpoint(ctx.harness, ctx.agent_id)
          send(notify, {:drained, msgs})
          loop(instructions, ctx, notify)

        :release ->
          {:ok, "final:" <> instructions}
      end
    end
  end

  defp start_harness(opts) do
    name = :"agent_harness_test_#{System.unique_integer([:positive])}"
    defaults = [name: name, runner: SleepyRunner, default_model: "stub-model"]
    start_supervised!({AgentHarness, Keyword.merge(defaults, opts)})
    name
  end

  defp create(harness, opts \\ []) do
    AgentHarness.create_subagent(harness, "instructions", Keyword.put(opts, :notify, self()))
  end

  test "create_subagent returns immediately while the runner keeps working" do
    harness = start_harness([])

    {elapsed_us, {:ok, id}} = :timer.tc(fn -> create(harness) end)

    # The runner blocks forever (until :finish); only the spawn is timed.
    assert elapsed_us < 500_000
    assert_receive {:started, _task, ^id, "instructions"}
    assert {:ok, :working} = AgentHarness.subagent_status(harness, id)
  end

  test "messages are delivered at the checkpoint after the next tool result, not before" do
    harness = start_harness(runner: CheckpointRunner)
    {:ok, id} = create(harness)
    assert_receive {:runner, task, ^id}

    # A tool result before any send drains nothing.
    send(task, :tool_result)
    assert_receive {:checkpoint, []}

    :ok = AgentHarness.send_message(harness, "lead", id, "hello")

    # The queued message surfaces only at the checkpoint after the next
    # tool result.
    send(task, :tool_result)
    assert_receive {:checkpoint, [msg]}
    assert %AgentHarness.Message{from: "lead", to: ^id, text: "hello", kind: :message} = msg

    send(task, :finish)
  end

  test "wait_for_message blocks until a message arrives" do
    harness = start_harness(runner: WaitingRunner)
    {:ok, id} = create(harness)
    assert_receive {:waiting, _task, ^id}

    refute_receive {:got, _msgs}, 100

    :ok = AgentHarness.send_message(harness, "lead", id, "wake up")
    assert_receive {:got, [msg]}
    assert msg.text == "wake up"
  end

  test "the final response reaches the lead, the agent idles, and a message wakes it" do
    harness = start_harness(runner: EchoRunner)
    {:ok, id} = create(harness)
    assert_receive {:run, ^id, "instructions"}

    assert {:ok, [final]} = AgentHarness.wait_for_message(harness, AgentHarness.lead_id(), 1_000)
    assert final.kind == :final
    assert final.from == id
    assert final.text == "answer:instructions"
    assert {:ok, :idle} = AgentHarness.subagent_status(harness, id)

    # Waking: a message to an idle subagent becomes its new instructions.
    :ok = AgentHarness.send_message(harness, "lead", id, "round two")
    assert_receive {:run, ^id, "round two"}
    assert {:ok, [final2]} = AgentHarness.wait_for_message(harness, AgentHarness.lead_id(), 1_000)
    assert final2.text == "answer:round two"
  end

  test "delete_subagent frees a concurrency slot" do
    harness = start_harness(max_concurrent: 2)

    {:ok, a} = create(harness)
    {:ok, _b} = create(harness)
    assert {:error, :max_concurrent} = create(harness)

    assert :ok = AgentHarness.delete_subagent(harness, a)
    assert {:ok, :terminated} = AgentHarness.subagent_status(harness, a)
    assert {:ok, _c} = create(harness)
  end

  test "max_total caps lifetime spawns even after deletes" do
    harness = start_harness(max_total: 2)

    {:ok, a} = create(harness)
    :ok = AgentHarness.delete_subagent(harness, a)
    {:ok, b} = create(harness)
    :ok = AgentHarness.delete_subagent(harness, b)

    assert {:error, :max_total} = create(harness)
  end

  test "per-agent token budget is enforced" do
    harness = start_harness([])
    {:ok, id} = create(harness, token_budget: 100)

    assert {:ok, 60} = AgentHarness.add_usage(harness, id, 40)
    assert {:error, :budget_exhausted} = AgentHarness.add_usage(harness, id, 100)
    assert {:ok, %{tokens_remaining: 0}} = AgentHarness.checkpoint(harness, id)
  end

  test "a runner crash while blocked in wait_for_message does not strand the agent" do
    harness = start_harness(runner: WaitingRunner)
    {:ok, id} = create(harness)
    assert_receive {:waiting, task, ^id}

    # Pin the race: the runner notifies before its wait call registers, so
    # wait for the agent to actually hold the waiter before killing it.
    agent = GenServer.whereis(AgentHarness.Agent.via(harness, id))
    wait_until(fn -> match?(%{waiters: [_ | _]}, :sys.get_state(agent)) end)

    Process.exit(task, :kill)

    # The crash surfaces to the lead as an :error final and the agent idles.
    assert {:ok, [msg]} = AgentHarness.wait_for_message(harness, AgentHarness.lead_id(), 1_000)
    assert msg.kind == :error
    assert msg.from == id
    assert {:ok, :idle} = AgentHarness.subagent_status(harness, id)

    # Regression (C2): the dead run's stale waiter must not swallow the
    # wake; the message has to start a fresh run.
    :ok = AgentHarness.send_message(harness, "lead", id, "wake")
    assert_receive {:waiting, _task2, ^id}
    assert {:ok, :working} = AgentHarness.subagent_status(harness, id)
  end

  test "a message landing during the final-composition window re-runs the agent" do
    harness = start_harness(runner: LatchRunner)
    {:ok, id} = create(harness)
    assert_receive {:run, task, ^id, "instructions"}

    # The agent is :working and past its last checkpoint; these queue.
    :ok = AgentHarness.send_message(harness, "lead", id, "follow-up")
    :ok = AgentHarness.send_message(harness, "lead", id, "second")

    send(task, :release)
    assert {:ok, [final]} = AgentHarness.wait_for_message(harness, AgentHarness.lead_id(), 1_000)
    assert final.text == "final:instructions"

    # Regression (C1): the queued head becomes the new instructions instead
    # of idling over it...
    assert_receive {:run, task2, ^id, "follow-up"}

    # ...and the rest of the queue stays FIFO for the next checkpoint.
    send(task2, :checkpoint)
    assert_receive {:drained, [msg]}
    assert msg.text == "second"

    send(task2, :release)
  end

  test "default subagent ids skip names the host already took" do
    harness = start_harness([])

    {:ok, "sub-2"} =
      AgentHarness.create_subagent(harness, "instructions", name: "sub-2", notify: self())

    # total_created is now 1, so the naive default for the next spawn would
    # be the taken "sub-2"; admission must pick a free id instead.
    assert {:ok, id} = create(harness)
    assert id != "sub-2"
    assert {:ok, :working} = AgentHarness.subagent_status(harness, id)
  end

  test "messaging an unknown agent reports not_found" do
    harness = start_harness([])
    assert {:error, :not_found} = AgentHarness.send_message(harness, "lead", "ghost", "hi")
  end

  defp wait_until(fun, tries \\ 50) do
    cond do
      fun.() ->
        :ok

      tries == 0 ->
        flunk("condition never became true")

      true ->
        Process.sleep(10)
        wait_until(fun, tries - 1)
    end
  end
end
