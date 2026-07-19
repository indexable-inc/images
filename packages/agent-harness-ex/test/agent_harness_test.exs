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
      send(notify, {:waiting, ctx.agent_id})
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
    assert_receive {:waiting, ^id}

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

  test "messaging an unknown agent reports not_found" do
    harness = start_harness([])
    assert {:error, :not_found} = AgentHarness.send_message(harness, "lead", "ghost", "hi")
  end
end
