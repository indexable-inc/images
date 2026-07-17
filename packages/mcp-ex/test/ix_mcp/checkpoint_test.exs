defmodule IxMcp.CheckpointTest do
  use ExUnit.Case, async: false

  alias IxMcp.Jobs

  setup do
    IxMcp.Workspace.reset()
    :ok
  end

  test "file checkpoint round-trips data and skips live references" do
    {_, _} = Jobs.run("keep_me = [1, 2, 3]", intent: "bind data")
    {_, _} = Jobs.run("me = self()", intent: "bind a pid")

    path = Path.join(System.tmp_dir!(), "ix-mcp-checkpoint-#{System.unique_integer([:positive])}")
    on_exit(fn -> File.rm(path) end)

    assert {:ok, skipped} = IxMcp.Checkpoint.save_file(path)
    assert :me in skipped

    IxMcp.Workspace.reset()
    IxMcp.Checkpoint.clear()
    assert {:ok, restored} = IxMcp.Checkpoint.load_file(path)
    assert restored >= 1

    # Workspace picks the checkpoint up on its next restart.
    IxMcp.Kernel.restart()
    {summary, _} = Jobs.run("keep_me", intent: "after load")
    assert summary.result == "[1, 2, 3]"
  end
end
