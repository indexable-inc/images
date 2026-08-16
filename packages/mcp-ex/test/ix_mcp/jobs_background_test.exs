defmodule IxMcp.JobsBackgroundTest do
  # The Jobs entry points agents actually guessed (error-report triage):
  # Jobs.spawn/2, Jobs.start/2, and Jobs.run(code, budget) with a bare
  # number. All are now real, documented forms.
  use ExUnit.Case, async: false

  alias IxMcp.Jobs

  setup do
    IxMcp.Workspace.reset()
    :ok
  end

  test "Jobs.spawn starts a background job and returns at once" do
    {summary, _out} = Jobs.spawn("Process.sleep(150); :spawned", intent: "spawn form")
    assert summary.running

    final = Jobs.await(summary.id, 5_000)
    assert final.status == :done
    assert final.result == ":spawned"
  end

  test "Jobs.start is the same background form" do
    {summary, _out} = Jobs.start("Process.sleep(150); :started", intent: "start form")
    assert summary.running
    assert Jobs.await(summary.id, 5_000).status == :done
  end

  test "Jobs.run accepts a bare number as the budget" do
    {summary, _out} = Jobs.run(":quick", 5)
    assert summary.status == :done
    assert summary.result == ":quick"
  end
end
