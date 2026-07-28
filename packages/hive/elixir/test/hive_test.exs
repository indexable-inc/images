defmodule HiveTest do
  use ExUnit.Case

  test "agents message each other by id" do
    {:ok, _} = Hive.spawn_agent(:a)
    {:ok, _} = Hive.spawn_agent(:b)

    Hive.Agent.whisper(:b, :a, :hello)
    Process.sleep(20)

    assert Hive.Agent.inbox(:b) == [{:a, :hello}]
  end

  test "broadcast reaches every other agent but not the sender" do
    {:ok, _} = Hive.spawn_agent(:x)
    {:ok, _} = Hive.spawn_agent(:y)
    {:ok, _} = Hive.spawn_agent(:z)

    Hive.Agent.broadcast(:x, :ping)
    Process.sleep(20)

    assert Hive.Agent.inbox(:y) == [{:x, :ping}]
    assert Hive.Agent.inbox(:z) == [{:x, :ping}]
    assert Hive.Agent.inbox(:x) == []
  end
end
