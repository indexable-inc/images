defmodule IxMcp.ParentTest do
  # async: false: mutates the env that marks a kernel as somebody's child.
  use ExUnit.Case, async: false

  alias IxMcp.Parent

  defp put_env(name, value) do
    System.put_env(name, value)
    on_exit(fn -> System.delete_env(name) end)
  end

  test "a lead has no parent, and says so instead of guessing a target" do
    refute Parent.child?()

    assert_raise RuntimeError, ~r/this kernel is a lead/, fn -> Parent.parent_session!() end
    assert_raise RuntimeError, ~r/this kernel is a lead/, fn -> Parent.id!() end
    assert_raise RuntimeError, ~r/IX_AGENT_PARENT_SESSION is unset/, fn -> Parent.send("hi") end
  end

  test "a child knows its parent and its own id" do
    put_env("IX_AGENT_PARENT_SESSION", "17")
    put_env("IX_AGENT_ID", "sub-3")

    assert Parent.child?()
    assert Parent.parent_session!() == 17
    assert Parent.id!() == "sub-3"
  end

  test "a junk parent id is loud rather than a message into the void" do
    put_env("IX_AGENT_PARENT_SESSION", "the-lead")

    assert_raise RuntimeError, ~r/not a session id/, fn -> Parent.parent_session!() end
  end

  test "messaging a parent that is not in the directory fails by name" do
    put_env("IX_AGENT_PARENT_SESSION", "2147483646")
    put_env("IX_AGENT_ID", "sub-9")

    assert {:error, reason} = Parent.send("half a finding")
    assert reason =~ "no session 2147483646"
  end
end
