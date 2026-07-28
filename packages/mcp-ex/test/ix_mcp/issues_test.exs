defmodule IxMcp.IssuesTest do
  use ExUnit.Case, async: false

  alias IxMcp.ActionLog
  alias IxMcp.Issues

  @moduletag :tmp_dir

  # Own ActionLog instance (in-memory, own name): claims must never land in
  # the globally running log, where a leaked IX_MCP_STDIO=1 application boot
  # has a real IssueWatch sweeping (see issue_watch_test.exs).
  setup do
    name = :"issues_test_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: ":memory:", name: name})
    %{log: name}
  end

  defp gh_stub(dir, script) do
    gh = Path.join(dir, "gh")
    File.write!(gh, "#!/bin/sh\n" <> script)
    File.chmod!(gh, 0o755)
    gh
  end

  test "won claim mirrors the assignee and reports the win", %{tmp_dir: dir, log: log} do
    args_log = Path.join(dir, "args.log")
    gh = gh_stub(dir, ~s(echo "$@" >> #{args_log}\n))
    sid = ActionLog.create_session("picker", log)

    assert {:ok, detail} =
             Issues.pickup("indexable-inc/index#4001", action_log: log, session_id: sid, gh: gh)

    assert detail =~ "claimed indexable-inc/index#4001"
    assert detail =~ "assigned @me"

    assert File.read!(args_log) =~
             "issue edit 4001 --repo indexable-inc/index --add-assignee @me"
  end

  test "second pickup loses and names the winner", %{tmp_dir: dir, log: log} do
    gh = gh_stub(dir, "exit 0\n")
    winner = ActionLog.create_session("winner", log)
    loser = ActionLog.create_session("loser", log)

    assert {:ok, _detail} =
             Issues.pickup("indexable-inc/index#4002",
               action_log: log,
               session_id: winner,
               gh: gh
             )

    assert {:error, message} =
             Issues.pickup("indexable-inc/index#4002", action_log: log, session_id: loser, gh: gh)

    assert message =~ "claimed by session winner at "
  end

  test "an integer claims on the default repo", %{log: log} do
    sid = ActionLog.create_session("picker", log)

    assert {:ok, detail} = Issues.pickup(4003, action_log: log, session_id: sid, gh: nil)
    assert detail =~ "claimed indexable-inc/index#4003"
    assert detail =~ "not mirrored"

    assert [%{kind: :issue, ref: "indexable-inc/index#4003", status: :claimed}] =
             ActionLog.list_requests(log)
  end

  test "a failed GitHub mirror does not lose a won claim", %{tmp_dir: dir, log: log} do
    gh = gh_stub(dir, "echo no auth >&2\nexit 1\n")
    sid = ActionLog.create_session("picker", log)

    assert {:ok, detail} =
             Issues.pickup("indexable-inc/index#4004", action_log: log, session_id: sid, gh: gh)

    assert detail =~ "GitHub assign failed"

    # The arbiter still holds the claim: a rival session's attempt loses,
    # while the claimant re-claiming reads back as its own standing win
    # (idempotent per session, so the client seam can retry a claim across
    # a server restart, #3903).
    rival = ActionLog.create_session("rival", log)

    assert {:error, message} =
             Issues.pickup("indexable-inc/index#4004", action_log: log, session_id: rival, gh: gh)

    assert message =~ "claimed by session picker at "

    assert {:ok, _detail} =
             Issues.pickup("indexable-inc/index#4004", action_log: log, session_id: sid, gh: gh)
  end

  test "a malformed ref is rejected without touching the arbiter", %{log: log} do
    assert {:error, message} = Issues.pickup("not-a-ref", action_log: log, gh: nil)
    assert message =~ ~s(pass an integer or "owner/repo#n")
    assert [] = ActionLog.list_requests(log)
  end
end
