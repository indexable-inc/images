defmodule IxMcp.AgentsTest do
  # async: false: mutates IX_AGENT_CHILD, which gates the shared surface.
  use ExUnit.Case, async: false

  alias IxMcp.Agents
  alias IxMcp.Agents.Events

  test "spawn is lead-only: refuses inside a child kernel" do
    System.put_env("IX_AGENT_CHILD", "1")
    on_exit(fn -> System.delete_env("IX_AGENT_CHILD") end)

    assert_raise RuntimeError, ~r/lead-only/, fn -> Agents.spawn("x") end
  end

  test "send targets only known children" do
    assert {:error, :not_found} = Agents.send("no-such-child", "hi")
  end

  test "await times out cleanly" do
    assert {:error, :timeout} = Events.await("never-spawned", 50)
  end

  test "registering a child with a session row stamps its heartbeat" do
    import IxMcpTest.Eventually

    alias IxMcp.ActionLog

    # A directory row the way Agents.spawn creates one: parented, never
    # heartbeat. The global Events instance owns the stamping.
    lead = ActionLog.create_session("beat-lead")
    child = ActionLog.create_session("beat-child", ActionLog, parent: lead)

    row = fn -> Enum.find(ActionLog.session_directory(), &(&1.id == child)) end
    assert %{last_seen_at: nil} = row.()

    Events.register_spawn("beat-agent-#{System.unique_integer([:positive])}", %{
      backend: :claude,
      model: "m",
      brief: "b",
      child_session: child
    })

    # The register cast is async; the stamp lands within the poll window.
    eventually(fn ->
      case row.() do
        %{last_seen_at: nil} -> nil
        %{last_seen_at: at} -> at
      end
    end)
  end

  test "a child registered without a session row is not stamped" do
    # The best-effort path: ActionLog was down at spawn, child_session nil.
    # Nothing to assert crashed is the assertion; a raise in the Events
    # server would fail the register that follows.
    Events.register_spawn("beatless-agent-#{System.unique_integer([:positive])}", %{
      backend: :claude,
      model: "m",
      brief: "b",
      child_session: nil
    })

    assert {:error, :timeout} = Events.await("still-alive-probe", 50)
  end
end
