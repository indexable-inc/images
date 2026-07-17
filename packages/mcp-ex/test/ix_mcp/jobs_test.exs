defmodule IxMcp.JobsTest do
  use ExUnit.Case, async: false

  alias IxMcp.Jobs

  setup do
    IxMcp.Workspace.reset()
    :ok
  end

  test "budget-then-background: a slow cell returns a running handle and finishes later" do
    {summary, _out} = Jobs.run("Process.sleep(300); :finally", budget: 0.05, intent: "slow")
    assert summary.running

    final = Jobs.await(summary.id, 5_000)
    assert final.status == :done
    assert final.result == ":finally"
  end

  test "a blocking cell never delays other jobs (the whole point)" do
    {blocked, _out} = Jobs.run("Process.sleep(:infinity)", budget: 0.05, intent: "block forever")
    assert blocked.running

    started = System.monotonic_time(:millisecond)
    {quick, _out} = Jobs.run("1 + 1", intent: "quick")
    elapsed = System.monotonic_time(:millisecond) - started

    assert quick.status == :done
    assert quick.result == "2"
    assert elapsed < 1_000

    assert :ok = Jobs.cancel(blocked.id)
    assert Jobs.get(blocked.id).status == :cancelled
  end

  test "output paging: tail, head, lines, slice, grep" do
    code = "for i <- 1..50, do: IO.puts(\"line \#{i}\")"
    {summary, _out} = Jobs.run(code, intent: "print lines")
    assert summary.status == :done

    assert Jobs.tail(summary.id, 2) =~ "line 50"
    assert Jobs.head(summary.id, 1) == "line 1"
    assert Jobs.lines(summary.id, 2, 3) == "line 2\nline 3"
    assert Jobs.slice(summary.id, 0, 2) == "line 1\nline 2"
    assert Jobs.grep(summary.id, ~r/line 4[89]/) == "line 48\nline 49"
    assert Jobs.grep(summary.id, "line 50") == "line 50"
  end

  test "result/1 returns the value term, or :running while unfinished" do
    {running, _out} = Jobs.run("Process.sleep(200); %{a: 1}", budget: 0.05, intent: "slow map")
    assert {:error, :running} = Jobs.result(running.id)

    Jobs.await(running.id, 5_000)
    assert {:ok, %{a: 1}} = Jobs.result(running.id)
  end

  test "history records runs with session and topic grouping" do
    IxMcp.Session.set_name("test session")
    IxMcp.Session.set_topic("test topic")

    {summary, _out} = Jobs.run(":recorded", intent: "history entry")
    entry = Enum.find(Jobs.history(10), &(&1.id == summary.id))

    assert entry.intent == "history entry"
    assert entry.session == "test session"
    assert entry.topic == "test topic"
    assert entry.status == :done
  end

  test "a crashed cell reports the exit reason, jobs registry survives" do
    {summary, _out} = Jobs.run("exit({:kaboom, %{state: 42}})", intent: "crash")
    assert summary.status == :failed
    assert summary.result =~ "kaboom"
    assert summary.result =~ "42"
  end

  @tag :os_procs
  test "cancelling a job kills OS processes its cell spawned (no orphans)" do
    marker = "ix-mcp-test-#{System.unique_integer([:positive])}"

    # A compound command keeps `sh` alive (a simple command would be exec'd,
    # dropping the marker from any argv pgrep can see).
    code = """
    System.cmd("sh", ["-c", "sleep 600; echo #{marker}"])
    """

    {summary, _out} = Jobs.run(code, budget: 0.2, intent: "spawn subprocess")
    assert summary.running

    # The subprocess exists while the job runs...
    assert {_, 0} = System.cmd("pgrep", ["-f", marker])

    assert :ok = Jobs.cancel(summary.id)
    Process.sleep(200)

    # ...and is gone, with its whole tree, after cancellation.
    assert {_, 1} = System.cmd("pgrep", ["-f", marker])
  end
end
