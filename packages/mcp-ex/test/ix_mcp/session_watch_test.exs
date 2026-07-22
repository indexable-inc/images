defmodule IxMcp.SessionWatchTest do
  use ExUnit.Case, async: false

  alias IxMcp.ActionLog
  alias IxMcp.MCP.Notifier
  alias IxMcp.SessionWatch

  # Own ActionLog instance and own watcher name: under a kernel-launched
  # shell IX_MCP_STDIO=1 leaks into mix test, so the application boot already
  # runs a global SessionWatch against the global log (see
  # issue_watch_test.exs); every assertion pins to this test's fixtures.
  setup do
    name = :"session_watch_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: ":memory:", name: name})
    %{log: name}
  end

  test "delivers addressed mail and broadcasts once, skipping pre-boot traffic", %{log: log} do
    me = ActionLog.create_session("watcher", log)
    sender = ActionLog.create_session("dispatcher", log)

    # Traffic standing before the watch boots is old news: the watermark
    # must swallow it (the claim feed's exact call, #3880).
    {:ok, _pre} = ActionLog.send_session_message(sender, nil, "pre-boot broadcast", log)

    Notifier.register(self())
    # register/1 is a cast; sync on the Notifier so the sweep's notify cast
    # cannot be processed ahead of the registration.
    _ = :sys.get_state(Notifier)

    start_supervised!(
      {SessionWatch,
       action_log: log, session_id: me, interval_ms: 25, name: :session_watch_under_test}
    )

    {:ok, _direct} = ActionLog.send_session_message(sender, me, "argazelka-fixture direct", log)
    {:ok, _blast} = ActionLog.send_session_message(sender, nil, "argazelka-fixture blast", log)

    assert_receive {:mcp_send,
                    %{
                      "method" => "notifications/claude/channel",
                      "params" =>
                        %{
                          "meta" => %{
                            "source" => "sessions",
                            "from" => "dispatcher (#" <> _,
                            "to" => "you"
                          }
                        } = direct_params
                    }},
                   5_000

    assert direct_params["content"] =~ "argazelka-fixture direct"
    assert direct_params["content"] =~ "message from session dispatcher"
    assert direct_params["content"] =~ "reply: Sessions.send(#{sender}"
    assert direct_params["meta"]["from_id"] == sender

    assert_receive {:mcp_send, %{"params" => %{"meta" => %{"to" => "broadcast"}} = blast_params}},
                   5_000

    assert blast_params["content"] =~ "argazelka-fixture blast"

    # Several more sweeps run inside this window; the watermark must swallow
    # both delivered rows and the pre-boot one instead of re-announcing.
    refute_receive {:mcp_send,
                    %{"params" => %{"meta" => %{"source" => "sessions", "from_id" => ^sender}}}},
                   200
  end

  test "never delivers the session's own sends", %{log: log} do
    me = ActionLog.create_session("soliloquist", log)

    Notifier.register(self())
    _ = :sys.get_state(Notifier)

    start_supervised!(
      {SessionWatch,
       action_log: log, session_id: me, interval_ms: 25, name: :session_watch_own_sends}
    )

    {:ok, _blast} = ActionLog.send_session_message(me, nil, "soliloquist-fixture", log)

    refute_receive {:mcp_send,
                    %{"params" => %{"meta" => %{"source" => "sessions", "from_id" => ^me}}}},
                   200
  end

  test "stamps the heartbeat at boot and keeps stamping on ticks", %{log: log} do
    me = ActionLog.create_session("beater", log)

    start_supervised!(
      {SessionWatch,
       action_log: log, session_id: me, interval_ms: 25, name: :session_watch_heartbeat}
    )

    assert %{last_seen_at: first} =
             ActionLog.session_directory(log) |> Enum.find(&(&1.id == me))

    assert {:ok, %DateTime{}, 0} = DateTime.from_iso8601(first)

    # A later tick moves the stamp forward.
    Process.sleep(50)

    assert %{last_seen_at: second} =
             ActionLog.session_directory(log) |> Enum.find(&(&1.id == me))

    assert second >= first
  end
end
