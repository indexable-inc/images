defmodule IxMcp.AgentsNotifyTest do
  # async: false: drives the one global Events server and the one Notifier.
  use ExUnit.Case, async: false

  alias IxMcp.ActionLog
  alias IxMcp.Agents
  alias IxMcp.Agents.Events
  alias IxMcp.MCP.Notifier
  alias IxMcp.Session

  # A subagent's final rides the same durable outbox a job finish rides
  # (#3839, #3934, #3700): before this, `settle` pushed the channel directly,
  # so a child that finished while no transport was attached was gone with the
  # CLI process that produced it.

  setup do
    # Let stragglers from earlier tests finish first: a job publishing into
    # this test's coalesce window would pool into its digests (#3934).
    for job <- IxMcp.Jobs.running(), do: IxMcp.Jobs.await(job.id, 10_000)

    # A fresh notifier per test, so transports and coalesce buffers from a
    # previous test cannot leak announcements into this one.
    :ok = Supervisor.terminate_child(IxMcp.Supervisor, Notifier)
    {:ok, _pid} = Supervisor.restart_child(IxMcp.Supervisor, Notifier)

    # Drain every leftover unacked row so registering a transport replays
    # nothing of its own.
    ActionLog.unacked_outbox() |> Enum.map(& &1.id) |> ActionLog.ack_outbox()
    :ok
  end

  test "a final becomes a durable outbox row scoped to the lead's session" do
    %{session_id: lead} = Session.ids()
    id = agent_id()
    register(id, :codex)
    final(id, "the answer")

    assert [row] = agent_rows(lead, id)
    assert row.source == :agents
    assert row.status == :done
    assert row.result == "the answer"
    assert row.session_id == lead
    # The backend rides as the row's label; it is what renders as [codex].
    assert row.intent == "codex"
    assert is_integer(row.elapsed_ms) and row.elapsed_ms >= 0

    # Durable does not mean "pretend to be a job": nothing was added to the
    # job ledger, so Jobs.history stays a history of jobs.
    refute Enum.any?(ActionLog.recent_jobs(lead, 50), &(&1.id == id))
  end

  test "a final that fires with no transport attached is replayed on reconnect" do
    %{session_id: lead} = Session.ids()
    id = agent_id()
    register(id)
    final(id, "work nobody was listening for")

    # Nothing could be delivered, so the row is still owed. This is the
    # no-silent-death invariant: the durable row, not a broadcast.
    assert [%{status: :done}] = agent_rows(lead, id)

    register_transport()

    assert_receive {:mcp_send, %{"params" => %{"content" => content, "meta" => meta}}}, 3_000
    assert content =~ "while you were away"
    assert content =~ id
    assert meta["source"] == "agents"
    assert_string_meta(meta)

    # Replayed once: the row was acked by the replay, so the coalesce flush
    # that follows finds nothing left to say.
    refute_receive {:mcp_send, _}, 2_500
    assert agent_rows(lead, id) == []
  end

  test "a live final renders with the agent_finished attributes a client keys on" do
    register_transport()
    id = agent_id()
    register(id)
    final(id, "rendered result")

    assert_receive {:mcp_send, %{"params" => %{"content" => content, "meta" => meta}}}, 3_000
    assert meta["source"] == "agents"
    assert meta["event"] == "agent_finished"
    assert meta["agent"] == id
    assert meta["status"] == "done"
    assert meta["backend"] == "claude"
    assert meta["severity"] == "info"
    assert content =~ "finished: done"
    # An agent's final text has no second home (unlike a job's output, which
    # stays behind Jobs.output), so it rides along even on a clean finish.
    assert content =~ "rendered result"
    assert_string_meta(meta)
  end

  test "a failed final announces as a failure, not as info" do
    register_transport()
    id = agent_id()
    register(id)
    error_final(id, "child died: exit 1")

    assert_receive {:mcp_send, %{"params" => %{"content" => content, "meta" => meta}}}, 3_000
    # The pre-durable path left severity to the Notifier's `info` default, so
    # a dead child announced itself as quietly as a clean one.
    assert meta["severity"] == "failure"
    assert meta["status"] == "failed"
    assert content =~ "child died: exit 1"
    assert_string_meta(meta)
  end

  test "await acks the final it returned, so it is not announced twice" do
    %{session_id: lead} = Session.ids()
    register_transport()
    id = agent_id()
    register(id)
    final(id, "awaited answer")

    assert {:ok, "awaited answer"} = Agents.await(id, 1_000)

    # The reply carried the outcome, so the announcement is suppressed the way
    # the exec reply path suppresses a job it waited out (#3934).
    assert agent_rows(lead, id) == []
    refute_receive {:mcp_send, _}, 2_500
  end

  test "a timed-out await acks nothing: the announcement is still owed" do
    %{session_id: lead} = Session.ids()
    id = agent_id()
    register(id)

    assert {:error, :timeout} = Agents.await(id, 100)

    final(id, "arrived after the caller gave up")
    assert [%{status: :done}] = agent_rows(lead, id)
  end

  test "expect_turn makes the next await block for the turn a message causes" do
    id = agent_id()
    register(id)
    final(id, "first turn")
    assert {:ok, "first turn"} = Events.await(id, 500)

    # Without this the await below returns "first turn" in 0.0s -- a confident
    # answer to the wrong question.
    :ok = Events.expect_turn(id)
    assert {:error, :timeout} = Events.await(id, 200)
    # And the child reports as having no result rather than a stale one.
    refute Map.has_key?(Agents.report(), id)

    final(id, "second turn")
    assert {:ok, "second turn"} = Events.await(id, 500)
  end

  test "send to a child the harness does not know leaves its stored final intact" do
    id = agent_id()
    register(id)
    final(id, "not to be lost")

    # Registered in the lead's ledger but never created in the harness, so the
    # existence check refuses -- and must refuse BEFORE clearing the final.
    assert {:error, :not_found} = Agents.send(id, "steer")
    assert {:ok, "not to be lost"} = Events.await(id, 500)
  end

  defp agent_id, do: "agent-notify-#{System.unique_integer([:positive])}"

  defp agent_rows(session_id, id) do
    for row <- ActionLog.unacked_outbox(session_id),
        row.source == :agents,
        row.ref == id,
        do: row
  end

  # Drive the seam the harness drain drives: a lead-mailbox message is the
  # only producer of a final, and `:sys.get_state` syncs the cast so the row
  # is on disk before the assertions read it.
  defp final(id, text), do: lead_message(%{kind: :final, from: id, text: text})

  defp error_final(id, text), do: lead_message(%{kind: :error, from: id, text: text})

  defp lead_message(msg) do
    GenServer.cast(Events, {:lead_message, msg})
    _ = :sys.get_state(Events)
    :ok
  end

  defp register(id, backend \\ :claude) do
    Events.register_spawn(id, %{backend: backend, model: "m", brief: "b", child_session: nil})
    _ = :sys.get_state(Events)
    :ok
  end

  defp register_transport do
    :ok = Notifier.register(self())
    _ = :sys.get_state(Notifier)
    :ok
  end

  # The client parses meta as string-to-string and drops the whole event on
  # anything else, so every frame this path can emit is checked.
  defp assert_string_meta(meta) do
    for {key, value} <- meta do
      assert is_binary(value), "meta #{inspect(key)} is #{inspect(value)}, not a string"
    end
  end
end
