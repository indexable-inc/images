defmodule IxMcp.NotifierTest do
  use ExUnit.Case, async: false

  import IxMcpTest.Eventually

  alias IxMcp.ActionLog
  alias IxMcp.Jobs
  alias IxMcp.MCP.Notifier
  alias IxMcp.MCP.Tools
  alias IxMcp.Session

  # -- #3934: announcements are scoped, suppressed when redundant, coalesced --

  setup do
    IxMcp.Workspace.reset()

    # Let stragglers from earlier tests finish first: a job publishing into
    # this test's coalesce window would pool into its digests (#3934).
    for job <- Jobs.running(), do: Jobs.await(job.id, 10_000)

    # A fresh notifier per test: transports, coalesce buffers, and watches
    # from a previous test must not leak announcements into this one.
    :ok = Supervisor.terminate_child(IxMcp.Supervisor, Notifier)
    {:ok, _pid} = Supervisor.restart_child(IxMcp.Supervisor, Notifier)

    # Drain every leftover unacked row so registering a transport here
    # replays nothing.
    ActionLog.unacked_outbox() |> Enum.map(& &1.id) |> ActionLog.ack_outbox()
    :ok
  end

  test "live delivery is scoped to the owning session's transports" do
    %{session_id: own} = Session.ids()
    other = ActionLog.create_session("sibling instance")

    register_transport()

    foreign = ledger_job(other, "foreign work")
    {:notify, foreign_outbox} = ActionLog.finish_job(foreign, :done, ":ok")
    Notifier.publish(foreign_outbox)

    ours = ledger_job(own, "own work")
    {:notify, our_outbox} = ActionLog.finish_job(ours, :done, ":ok")
    Notifier.publish(our_outbox)

    assert_receive {:mcp_send, %{"params" => %{"content" => content}}}, 2_000
    assert content =~ ours
    refute content =~ foreign
    refute_receive {:mcp_send, _}, 400

    # The sibling's row was neither delivered nor acked: it waits, durable,
    # for the sibling's own transport to replay it.
    assert Enum.any?(ActionLog.unacked_outbox(other), &(&1.job_id == foreign))
  end

  test "a job that finishes within its exec budget is not announced" do
    register_transport()

    {:ok, reply} = Tools.call("exec", %{"code" => ":done_fast", "intent" => "sync cell"})
    assert reply =~ ~s("status":"done")

    # The reply carried the outcome, so the reply path acked the outbox row
    # and the coalesce flush finds nothing to say.
    %{"job" => id} = reply |> String.split("\n") |> hd() |> JSON.decode!()
    refute Enum.any?(ActionLog.unacked_outbox(), &(&1.job_id == id))
    refute_receive {:mcp_send, _}, 600
  end

  test "a backgrounded job's failure always announces, loudly" do
    register_transport()

    {summary, _out} =
      Jobs.run(~S|Process.sleep(150); raise "boom-3934"|, budget: 0.05, intent: "doomed")

    assert summary.running
    id = summary.id

    assert_receive {:mcp_send, %{"params" => %{"content" => content, "meta" => meta}}}, 3_000
    assert content =~ id
    assert content =~ "boom-3934"
    assert meta["status"] == "failed"
    assert meta["severity"] == "failure"
  end

  test "several finishes for one session coalesce into a single digest" do
    %{session_id: own} = Session.ids()
    register_transport()

    a = ledger_job(own, "digest a")
    b = ledger_job(own, "digest b")
    {:notify, outbox_a} = ActionLog.finish_job(a, :done, ":a")
    {:notify, outbox_b} = ActionLog.finish_job(b, :failed, "went sideways")
    Notifier.publish(outbox_a)
    Notifier.publish(outbox_b)

    assert_receive {:mcp_send, %{"params" => %{"content" => content, "meta" => meta}}}, 2_000
    assert content =~ a
    assert content =~ b
    assert content =~ "went sideways"
    assert meta["digest"] == "2"
    # One failure makes the whole digest loud.
    assert meta["severity"] == "failure"
    refute_receive {:mcp_send, _}, 400

    # Delivered means acked: neither row replays later.
    refute Enum.any?(ActionLog.unacked_outbox(), &(&1.job_id in [a, b]))
  end

  test "an await wrapper announces nothing of its own" do
    register_transport()

    {target, _out} = Jobs.run("Process.sleep(400); :t", budget: 0.05, intent: "await target")
    assert target.running

    {wrapper, _out} =
      Jobs.run(~s|Jobs.await("#{target.id}")|, budget: 0.05, intent: "await wrapper")

    assert wrapper.running

    assert_receive {:mcp_send, %{"params" => %{"content" => content}}}, 3_000
    assert content =~ target.id
    refute content =~ wrapper.id

    # The wrapper's terminal transition is on the record -- born acked, so
    # it can never announce or replay. Poll for the write first: the
    # wrapper finishes moments after its target, not synchronously with it.
    assert %{status: :done} =
             eventually(fn ->
               case ActionLog.job(wrapper.id) do
                 %{status: :done} = job -> job
                 _not_yet -> nil
               end
             end)

    refute Enum.any?(ActionLog.unacked_outbox(), &(&1.job_id == wrapper.id))
    refute_receive {:mcp_send, _}, 600
  end

  test "Jobs.watch(job_id) announces another session's job here, without acking it" do
    other = ActionLog.create_session("watched sibling")
    id = ledger_job(other, "watched job")

    register_transport()
    :ok = Jobs.watch(id)

    # The owning instance would publish to its own transports; this side
    # only sees the shared ledger, which the watch polls.
    {:notify, _outbox} = ActionLog.finish_job(id, :done, ":ok")

    assert_receive {:mcp_send, %{"params" => %{"content" => content, "meta" => meta}}}, 3_000
    assert content =~ id
    assert meta["watch"] == "1"
    assert meta["severity"] == "info"

    # The row is the owning session's to deliver and ack, not the watcher's.
    assert Enum.any?(ActionLog.unacked_outbox(other), &(&1.job_id == id))
  end

  test "Jobs.watch(session: id) announces that session's future finishes only" do
    other = ActionLog.create_session("watched session")
    register_transport()

    old = ledger_job(other, "old news")
    {:notify, _outbox} = ActionLog.finish_job(old, :done, ":ok")

    :ok = Jobs.watch(session: other)

    fresh = ledger_job(other, "fresh work")
    {:notify, _outbox} = ActionLog.finish_job(fresh, :failed, "died abroad")

    assert_receive {:mcp_send, %{"params" => %{"content" => content, "meta" => meta}}}, 3_000
    assert content =~ fresh
    refute content =~ old
    assert meta["severity"] == "failure"
  end

  test "watch argument errors: unknown job, own session" do
    assert_raise ArgumentError, ~r/no such job/, fn -> Jobs.watch("zzzzzzzz") end

    %{session_id: own} = Session.ids()
    assert_raise ArgumentError, ~r/already announce/, fn -> Jobs.watch(session: own) end
  end

  test "channel messages default to info severity" do
    register_transport()
    Notifier.channel("hello there", %{"source" => "test"})

    assert_receive {:mcp_send,
                    %{"params" => %{"meta" => %{"severity" => "info", "source" => "test"}}}},
                   1_000
  end

  # The client renders meta as tag attributes and drops any event carrying a
  # value it cannot render, saying nothing to the sender: an integer count or
  # a nil ref cost the whole notification. Three of the kernel's own frames
  # shipped that way -- the coalesced digest, the reconnect replay, and every
  # cross-session watch hit -- so the shape is asserted here, not assumed.
  test "channel refuses a meta value the wire cannot carry" do
    register_transport()

    assert_raise ArgumentError, ~r/meta values must be strings/, fn ->
      Notifier.channel("counted", %{"source" => "test", "digest" => 2})
    end

    assert_raise ArgumentError, ~r/meta values must be strings/, fn ->
      Notifier.channel("absent ref", %{"source" => "test", "ref" => nil})
    end

    refute_receive {:mcp_send, _}, 200
  end

  test "every frame the notifier sends carries string meta values" do
    %{session_id: own} = Session.ids()
    register_transport()

    a = ledger_job(own, "wire a")
    b = ledger_job(own, "wire b")
    {:notify, outbox_a} = ActionLog.finish_job(a, :done, ":a")
    {:notify, outbox_b} = ActionLog.finish_job(b, :done, ":b")
    Notifier.publish(outbox_a)
    Notifier.publish(outbox_b)

    assert_receive {:mcp_send, %{"params" => %{"meta" => digest}}}, 2_000
    assert_wire_meta(digest)

    # The replay digest a reconnecting transport gets is the same shape.
    replayed = ledger_job(own, "wire replayed")
    {:notify, _outbox} = ActionLog.finish_job(replayed, :done, ":c")
    register_transport()

    assert_receive {:mcp_send, %{"params" => %{"meta" => replay}}}, 2_000
    assert_wire_meta(replay)
  end

  defp assert_wire_meta(meta) do
    for {key, value} <- meta do
      assert is_binary(value), "meta #{inspect(key)} is #{inspect(value)}, not a string"
    end
  end

  # Register the test process as this session's transport and sync on the
  # notifier, so registration (and its replay) is complete before the test
  # publishes anything.
  defp register_transport do
    :ok = Notifier.register(self())
    _ = :sys.get_state(Notifier)
    :ok
  end

  # A job that exists only in the ledger, as if a sibling kernel instance ran
  # it: `finish_job` on it produces a real outbox row without any live
  # process publishing or suppressing it.
  defp ledger_job(session_id, intent) do
    id = "nt" <> Base.encode16(:crypto.strong_rand_bytes(3), case: :lower)

    :ok =
      ActionLog.job_started(%{
        id: id,
        session_id: session_id,
        action_id: nil,
        intent: intent,
        session_name: nil,
        topic_name: nil,
        code: ":ok",
        watch: false,
        started_at: DateTime.to_iso8601(DateTime.utc_now())
      })

    id
  end
end
