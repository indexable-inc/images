defmodule IxMcp.WorkspaceIsolationTest do
  # Named workspaces: per-agent REPL isolation on one kernel (#3967
  # follow-up). Cells targeting different workspace names must never see
  # each other's bindings; the unnamed default stays "main" and behaves
  # exactly as the shared workspace always did.
  use ExUnit.Case, async: false

  alias IxMcp.Jobs
  alias IxMcp.MCP.Tools
  alias IxMcp.Workspace

  setup do
    Workspace.reset(Workspace.main())
    for name <- Workspace.named(), do: Workspace.drop(name)
    :ok
  end

  defp exec(code, args \\ %{}) do
    {:ok, reply} =
      Tools.call("exec", Map.merge(%{"code" => code, "intent" => "isolation test"}, args))

    reply
  end

  test "bindings in one named workspace are invisible to another and to main" do
    exec("secret = :a_only", %{"workspace" => "agent-a"})

    reply_b = exec("secret", %{"workspace" => "agent-b"})
    assert reply_b =~ ~s("status":"failed")
    assert reply_b =~ "undefined variable \"secret\""

    reply_main = exec("secret")
    assert reply_main =~ ~s("status":"failed")

    reply_a = exec("secret", %{"workspace" => "agent-a"})
    assert reply_a =~ ~s("status":"done")
    assert reply_a =~ "=> :a_only"
  end

  test "the same name is the same REPL across calls, and the reply names it" do
    exec("n = 1", %{"workspace" => "counter"})
    reply = exec("n = n + 1", %{"workspace" => "counter"})
    assert reply =~ ~s("workspace":"counter")
    assert reply =~ "=> 2"
  end

  test "unnamed calls stay on main and never mention a workspace" do
    exec("shared = :main_value")
    reply = exec("shared")
    assert reply =~ "=> :main_value"
    refute reply =~ ~s("workspace")
  end

  test "an invalid workspace name is refused before any job runs" do
    assert {:error, message} =
             Tools.call("exec", %{
               "code" => "1",
               "intent" => "bad name",
               "workspace" => "no spaces allowed"
             })

    assert message =~ "invalid workspace name"
  end

  test "Workspace.new/list/drop manage named workspaces from a cell" do
    {summary, _out} = Jobs.run(~s|Workspace.new("scratch")|, intent: "create")
    assert summary.status == :done
    assert summary.result =~ "created: true"

    {summary, _out} = Jobs.run("Workspace.list()", intent: "list")
    assert summary.result =~ ~s("main")
    assert summary.result =~ ~s("scratch")

    {summary, _out} = Jobs.run(~s|Workspace.drop("scratch")|, intent: "drop")
    assert summary.status == :done
    assert Workspace.named() == []
  end

  test "main cannot be dropped" do
    {summary, _out} = Jobs.run(~s|Workspace.drop("main")|, intent: "drop main")
    assert summary.status == :failed
    assert summary.result =~ "cannot be dropped"
  end

  test "cells default in-language workspace calls to their own workspace" do
    exec("mine = :here", %{"workspace" => "introspective"})
    reply = exec("Ix.bindings() |> Enum.map(& &1.name)", %{"workspace" => "introspective"})
    assert reply =~ ":mine"

    # The same introspection from main does not see the named workspace's
    # binding.
    reply_main = exec("Ix.bindings() |> Enum.map(& &1.name)")
    refute reply_main =~ ":mine"
  end

  test "a named workspace's bindings survive its process dying (checkpoint restore)" do
    exec("durable = 42", %{"workspace" => "phoenix"})

    [{pid, _}] = Registry.lookup(IxMcp.Workspaces.Registry, "phoenix")
    ref = Process.monitor(pid)
    Process.exit(pid, :kill)
    assert_receive {:DOWN, ^ref, :process, ^pid, :killed}, 2_000

    # The supervisor restarts it (or ensure/1 does); either way the
    # checkpoint row named "phoenix" restores its own bindings only.
    reply = exec("durable", %{"workspace" => "phoenix"})
    assert reply =~ "=> 42"
    refute :durable in Workspace.names(Workspace.main())
  end

  test "Ix.restart restores every workspace from its own checkpoint" do
    exec("kept_main = :m")
    exec("kept_named = :n", %{"workspace" => "resilient"})

    {summary, _out} = Jobs.run("Ix.restart()", intent: "restart")
    assert summary.status == :done
    assert summary.result =~ ~s("resilient")

    assert :kept_main in Workspace.names(Workspace.main())
    assert :kept_named in Workspace.names("resilient")
    refute :kept_named in Workspace.names(Workspace.main())
  end
end
