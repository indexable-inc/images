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
    assert direct_params["meta"]["from_id"] == Integer.to_string(sender)

    assert_receive {:mcp_send, %{"params" => %{"meta" => %{"to" => "broadcast"}} = blast_params}},
                   5_000

    assert blast_params["content"] =~ "argazelka-fixture blast"

    # Several more sweeps run inside this window; the watermark must swallow
    # both delivered rows and the pre-boot one instead of re-announcing.
    sender_id = Integer.to_string(sender)

    refute_receive {:mcp_send,
                    %{"params" => %{"meta" => %{"source" => "sessions", "from_id" => ^sender_id}}}},
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

    me_id = Integer.to_string(me)

    refute_receive {:mcp_send,
                    %{"params" => %{"meta" => %{"source" => "sessions", "from_id" => ^me_id}}}},
                   200
  end

  test "delivers the request feed once, skipping pre-boot events (#3883)", %{log: log} do
    me = ActionLog.create_session("request-watcher", log)
    poster = ActionLog.create_session("request-poster", log)
    worker = ActionLog.create_session("request-worker", log)

    # An event standing before the watch boots is old news: the watermark
    # must swallow it (the claim feed's exact call, #3880).
    {:ok, _pre} = ActionLog.post_request(:adhoc, nil, "pre-boot offer", nil, poster, log)

    Notifier.register(self())
    _ = :sys.get_state(Notifier)

    start_supervised!(
      {SessionWatch,
       action_log: log, session_id: me, interval_ms: 25, name: :session_watch_requests}
    )

    {:ok, request} =
      ActionLog.post_request(:adhoc, nil, "argazelka-fixture offer", "the body", poster, log)

    rid = Integer.to_string(request.id)

    assert_receive {:mcp_send,
                    %{
                      "method" => "notifications/claude/channel",
                      "params" =>
                        %{
                          "meta" => %{
                            "source" => "requests",
                            "event" => "posted",
                            "request" => ^rid
                          }
                        } = posted_params
                    }},
                   5_000

    assert posted_params["meta"]["session"] == "request-poster"
    assert posted_params["meta"]["kind"] == "adhoc"
    assert posted_params["content"] =~ "request posted: ##{request.id} argazelka-fixture offer"
    assert posted_params["content"] =~ "the body"
    assert posted_params["content"] =~ "pickup: Requests.pickup(#{request.id})"

    # The claim and the finish follow on the same feed, actor named.
    {:ok, _claimed} = ActionLog.claim_request(request.id, worker, log)
    {:ok, _done} = ActionLog.finish_request(request.id, worker, log)

    assert_receive {:mcp_send,
                    %{
                      "params" =>
                        %{"meta" => %{"event" => "claimed", "request" => ^rid}} =
                          claimed_params
                    }},
                   5_000

    assert claimed_params["meta"]["session"] == "request-worker"
    assert claimed_params["content"] =~ "request claimed: ##{request.id}"

    assert_receive {:mcp_send,
                    %{"params" => %{"meta" => %{"event" => "done", "request" => ^rid}}}},
                   5_000

    # Several more sweeps run inside this window; the watermark must swallow
    # the delivered events and the pre-boot one instead of re-announcing.
    refute_receive {:mcp_send,
                    %{
                      "params" => %{
                        "meta" => %{"source" => "requests", "session" => "request-poster"}
                      }
                    }},
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
