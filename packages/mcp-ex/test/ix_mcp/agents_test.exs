defmodule IxMcp.AgentsTest do
  # async: false: mutates IX_AGENT_DEPTH and IX_AGENT_MAX_DEPTH, which gate the
  # shared surface.
  use ExUnit.Case, async: false

  alias IxMcp.Agents
  alias IxMcp.Agents.Events

  defp put_env(name, value) do
    System.put_env(name, value)
    on_exit(fn -> System.delete_env(name) end)
  end

  test "a lead is depth 0 and its children are depth 1" do
    assert Agents.depth() == 0
    assert Agents.child_depth() == 1
    assert Agents.max_depth() == 2
  end

  test "spawn refuses at the cap, naming both numbers" do
    put_env("IX_AGENT_DEPTH", "2")

    assert_raise RuntimeError, ~r/depth 2 of a tree capped at 2/, fn -> Agents.spawn("x") end
  end

  test "the cap is raisable on the lead, and one below it still spawns" do
    put_env("IX_AGENT_DEPTH", "2")
    put_env("IX_AGENT_MAX_DEPTH", "3")

    # Past the gate: spawning is admitted. The child then dies on its missing
    # binary inside its own runner task, which is the harness's business and
    # reaches the lead as an error rather than raising here.
    assert {:ok, "depth-probe"} =
             Agents.spawn("x", bin: "/nonexistent/claude", name: "depth-probe")

    assert {:error, _reason} = Agents.await("depth-probe", 5_000)
    Agents.delete("depth-probe")
  end

  test "a junk depth is loud rather than treated as a lead" do
    put_env("IX_AGENT_DEPTH", "deep")

    assert_raise RuntimeError, ~r/IX_AGENT_DEPTH is "deep"/, fn -> Agents.depth() end
  end

  test "send targets only known children" do
    assert {:error, :not_found} = Agents.send("no-such-child", "hi")
  end

  test "interrupt says which way it failed" do
    assert {:error, :not_running} = Agents.interrupt("no-such-child")
  end

  test "await times out cleanly" do
    assert {:error, :timeout} = Events.await("never-spawned", 50)
  end
end
