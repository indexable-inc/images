defmodule IxMcp.ChaosTest do
  @moduledoc """
  The failure classes that motivated leaving Python, exercised live: a cell
  blocks forever while other jobs keep running; we trace it from outside,
  then restart the evaluator and the bindings come back.
  """

  use ExUnit.Case, async: false

  alias IxMcp.Jobs

  setup do
    IxMcp.Workspace.reset()
    :ok
  end

  test "wedged cell: others run, trace sees it, restart recovers bindings" do
    # State that must survive the storm.
    {bound, _} = Jobs.run("precious = %{answer: 42}", intent: "bind state")
    assert bound.status == :done

    # A cell wedges forever -- the Python equivalent froze every session.
    {wedged, _} = Jobs.run("Process.sleep(:infinity)", budget: 0.05, intent: "wedge")
    assert wedged.running

    # Other jobs are unaffected.
    {ok, _} = Jobs.run("precious.answer * 2", intent: "keep working")
    assert ok.result == "84"

    # Trace from outside shows the wedged frame while it is still wedged.
    trace = IxMcp.Kernel.trace()
    assert trace =~ wedged.id
    assert trace =~ "sleep"

    # Restart: the wedged job dies, bindings restore from the checkpoint.
    report = IxMcp.Kernel.restart()
    assert wedged.id in report.jobs_cancelled
    assert report.bindings_restored >= 1
    assert Jobs.get(wedged.id).status == :cancelled

    {after_restart, _} = Jobs.run("precious.answer", intent: "check restore")
    assert after_restart.status == :done
    assert after_restart.result == "42"
  end

  @tag :os_procs
  test "a job cancelled mid-subprocess leaves no orphan even under restart" do
    marker = "ix-mcp-chaos-#{System.unique_integer([:positive])}"

    code = """
    System.cmd("sh", ["-c", "sleep 600; echo #{marker}"])
    """

    {job, _} = Jobs.run(code, budget: 0.2, intent: "spawn subprocess")

    assert job.running
    assert {_, 0} = System.cmd("pgrep", ["-f", marker])

    IxMcp.Kernel.restart()
    Process.sleep(200)

    assert {_, 1} = System.cmd("pgrep", ["-f", marker])
  end

  test "Ix.restart() from a cell recovers a wedged evaluator, sparing its own cell" do
    {bound, _} = Jobs.run("precious = 1", intent: "bind state")
    assert bound.status == :done

    {wedged, _} = Jobs.run("Process.sleep(:infinity)", budget: 0.05, intent: "wedge")
    assert wedged.running

    # The prelude aliases IxMcp.Kernel as Ix, so recovery is reachable from
    # any fresh cell even while another job wedges -- and the requesting
    # cell survives its own restart to return the report.
    {restart, _} = Jobs.run("Ix.restart()", intent: "restart from a cell")
    assert restart.status == :done
    assert restart.result =~ wedged.id
    assert Jobs.get(wedged.id).status == :cancelled

    {check, _} = Jobs.run("precious", intent: "after in-cell restart")
    assert check.status == :done
    assert check.result == "1"
  end

  test "workspace crash restores bindings via supervisor + checkpoint (no tool call needed)" do
    {_, _} = Jobs.run("phoenix = :rises", intent: "bind")

    Process.exit(Process.whereis(IxMcp.Workspace), :kill)
    Process.sleep(100)

    {summary, _} = Jobs.run("phoenix", intent: "after crash")
    assert summary.status == :done
    assert summary.result == ":rises"
  end
end
