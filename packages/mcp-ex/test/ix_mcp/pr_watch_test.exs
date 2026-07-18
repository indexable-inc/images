defmodule IxMcp.PrWatchTest do
  use ExUnit.Case, async: false

  alias IxMcp.MCP.Notifier
  alias IxMcp.PrWatch

  # The dying watcher's crash report is expected noise here.
  @moduletag :capture_log

  # Regression for #3553: IX_MCP_GH points at a gh that passes the start-time
  # guard but cannot exec -- the shape of a release whose runtime PATH lost
  # gh. The watcher crashes, and the promised channel notification must still
  # arrive as a loud error instead of dying silently in the supervisor logs.
  test "a crashed watcher still notifies the channel" do
    previous = System.get_env("IX_MCP_GH")
    System.put_env("IX_MCP_GH", "/nonexistent/gh")

    on_exit(fn ->
      if previous, do: System.put_env("IX_MCP_GH", previous), else: System.delete_env("IX_MCP_GH")
    end)

    Notifier.register(self())
    # register/1 is a cast; sync on the Notifier so the watcher's own notify
    # cast cannot be processed ahead of the registration.
    _ = :sys.get_state(Notifier)

    assert {:ok, _} = PrWatch.start("1", File.cwd!())

    assert_receive {:mcp_send, %{"method" => "notifications/message", "params" => params}}, 5_000
    assert %{"level" => "error", "logger" => "ix_mcp.pr_watch", "data" => data} = params
    assert %{"event" => "pr_watch", "pr" => "1", "state" => "error"} = data
    assert data["detail"] =~ "enoent"
  end
end
