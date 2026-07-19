defmodule IxMcp.AgentsTest do
  # async: false: mutates IX_AGENT_CHILD, which gates the shared surface.
  use ExUnit.Case, async: false

  test "spawn is lead-only: refuses inside a child kernel" do
    System.put_env("IX_AGENT_CHILD", "1")
    on_exit(fn -> System.delete_env("IX_AGENT_CHILD") end)

    assert_raise RuntimeError, ~r/lead-only/, fn -> IxMcp.Agents.spawn("x") end
  end

  test "send targets only known children" do
    assert {:error, :not_found} = IxMcp.Agents.send("no-such-child", "hi")
  end

  test "await times out cleanly" do
    assert {:error, :timeout} = IxMcp.Agents.Events.await("never-spawned", 50)
  end
end
