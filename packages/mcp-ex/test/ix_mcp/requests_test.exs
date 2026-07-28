defmodule IxMcp.RequestsTest do
  use ExUnit.Case, async: false

  alias IxMcp.ActionLog
  alias IxMcp.Requests

  # Own ActionLog instance (in-memory, own name): requests must never land
  # in the globally running log, where a leaked IX_MCP_STDIO=1 application
  # boot has a real SessionWatch sweeping (see issue_watch_test.exs).
  setup do
    name = :"requests_test_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: ":memory:", name: name})
    %{log: name}
  end

  test "post, pickup, done walk the lifecycle; the board lists open first", %{log: log} do
    poster = ActionLog.create_session("poster", log)
    worker = ActionLog.create_session("worker", log)

    assert {:ok, detail} =
             Requests.post("review the diff", "the body", action_log: log, session_id: poster)

    assert detail =~ "posted request #"

    assert [%{id: id, title: "review the diff", body: "the body", status: :open}] =
             Requests.list(action_log: log)

    assert {:ok, detail} = Requests.pickup(id, action_log: log, session_id: worker)
    assert detail =~ "claimed request ##{id} (review the diff) at "

    # The board sorts the still-open offer ahead of the claimed one.
    assert {:ok, _detail} =
             Requests.post("second offer", nil, action_log: log, session_id: poster)

    assert [%{title: "second offer", status: :open}, %{id: ^id, status: :claimed}] =
             Requests.list(action_log: log)

    assert {:ok, detail} = Requests.done(id, action_log: log, session_id: worker)
    assert detail =~ "request ##{id} (review the diff) done at "
  end

  test "a lost pickup names the winner; finishing open work is refused", %{log: log} do
    poster = ActionLog.create_session("poster", log)
    winner = ActionLog.create_session("winner", log)
    loser = ActionLog.create_session("loser", log)

    {:ok, _detail} = Requests.post("contended", nil, action_log: log, session_id: poster)
    [%{id: id}] = Requests.list(action_log: log)

    assert {:ok, _detail} = Requests.pickup(id, action_log: log, session_id: winner)
    assert {:error, message} = Requests.pickup(id, action_log: log, session_id: loser)
    assert message =~ "claimed by session winner at "

    {:ok, _detail} = Requests.post("untouched", nil, action_log: log, session_id: poster)
    [%{id: open_id, status: :open} | _rest] = Requests.list(action_log: log)

    assert {:error, message} = Requests.done(open_id, action_log: log, session_id: winner)
    assert message =~ "still open"

    assert {:error, message} = Requests.pickup(999_999, action_log: log, session_id: loser)
    assert message =~ "no request #999999"
  end

  test "an issue-kind post needs a well-formed ref and ensures idempotently", %{log: log} do
    poster = ActionLog.create_session("poster", log)

    assert {:error, message} = Requests.post("t", nil, kind: :issue, action_log: log)
    assert message =~ ~s(needs ref: "owner/repo#n")

    assert {:error, message} =
             Requests.post("t", nil, kind: :issue, ref: "not-a-ref", action_log: log)

    assert message =~ "unrecognized issue ref"

    assert {:error, message} = Requests.post("t", nil, ref: "owner/repo#1", action_log: log)
    assert message =~ "needs kind: :issue"

    # An issue-kind post is an ensure, so its detail reports standing state
    # rather than claiming a fresh offer.
    assert {:ok, detail} =
             Requests.post("take over the PR", nil,
               kind: :issue,
               ref: "indexable-inc/index#3883",
               action_log: log,
               session_id: poster
             )

    assert detail =~ "for indexable-inc/index#3883: open, posted by session poster"

    # Re-posting the same ref reads the standing row back instead of
    # offering the work twice.
    assert {:ok, detail} =
             Requests.post("another title", nil,
               kind: :issue,
               ref: "indexable-inc/index#3883",
               action_log: log,
               session_id: poster
             )

    assert detail =~ "open, posted by session poster"
    assert [%{title: "take over the PR"}] = Requests.list(action_log: log)
  end
end
