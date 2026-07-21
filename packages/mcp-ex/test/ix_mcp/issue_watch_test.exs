defmodule IxMcp.IssueWatchTest do
  use ExUnit.Case, async: false

  alias IxMcp.ActionLog
  alias IxMcp.IssueWatch
  alias IxMcp.MCP.Notifier

  @moduletag :tmp_dir

  test "empty owners disables the watcher" do
    assert :ignore = IssueWatch.start_link(owners: [], name: nil)
  end

  test "announces a newly filed issue once", %{tmp_dir: dir} do
    created = DateTime.utc_now() |> DateTime.add(60) |> DateTime.to_iso8601()

    fixture = Path.join(dir, "issues.json")

    File.write!(
      fixture,
      JSON.encode!([
        %{
          "number" => 3877,
          "title" => "kernel: notify on issue filing",
          "url" => "https://github.com/indexable-inc/index/issues/3877",
          "repository" => %{"nameWithOwner" => "indexable-inc/index"},
          "author" => %{"login" => "andrewgazelka"},
          "createdAt" => created
        }
      ])
    )

    # A gh stand-in that answers every search with the same fixture: the
    # first sweep must announce the issue, every later sweep must dedupe it.
    gh = Path.join(dir, "gh")
    File.write!(gh, "#!/bin/sh\nexec cat #{fixture}\n")
    File.chmod!(gh, 0o755)

    Notifier.register(self())
    # register/1 is a cast; sync on the Notifier so the sweep's notify cast
    # cannot be processed ahead of the registration.
    _ = :sys.get_state(Notifier)

    # Own name: under a kernel-launched shell IX_MCP_STDIO=1 leaks into
    # mix test, so the application boot already registered the global
    # IssueWatch and the default name collides.
    start_supervised!(
      {IssueWatch,
       gh: gh, owners: ["indexable-inc"], interval_ms: 25, name: :issue_watch_under_test}
    )

    # Pin to the fixture's ref: a globally started IssueWatch (see above)
    # could announce a real issue in this window.
    assert_receive {:mcp_send,
                    %{
                      "method" => "notifications/claude/channel",
                      "params" =>
                        %{
                          "meta" => %{"source" => "issues", "issue" => "indexable-inc/index#3877"}
                        } =
                          params
                    }},
                   5_000

    assert %{"content" => content, "meta" => meta} = params
    assert meta["author"] == "andrewgazelka"
    assert content =~ "kernel: notify on issue filing"
    assert content =~ "issues/3877"

    # Several more sweeps run inside this window; the seen set must swallow
    # the unchanged search result instead of re-announcing it.
    refute_receive {:mcp_send,
                    %{"params" => %{"meta" => %{"issue" => "indexable-inc/index#3877"}}}},
                   200
  end

  test "announces a pickup claim once, skipping claims that predate boot", %{tmp_dir: dir} do
    # Own ActionLog instance: the sweep must be driven by this test's claims
    # alone, not whatever the globally running log accumulates.
    log = :"issue_watch_claims_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: ":memory:", name: log})

    # gh answers every issue search empty; only the claims sweep speaks.
    gh = Path.join(dir, "gh")
    File.write!(gh, "#!/bin/sh\necho '[]'\n")
    File.chmod!(gh, 0o755)

    stale = ActionLog.create_session("stale", log)
    {:ok, _claim} = ActionLog.claim_issue("indexable-inc/index", 4200, stale, log)

    Notifier.register(self())
    _ = :sys.get_state(Notifier)

    start_supervised!(
      {IssueWatch,
       gh: gh,
       owners: ["indexable-inc"],
       interval_ms: 25,
       name: :issue_watch_claims_under_test,
       action_log: log}
    )

    claimer = ActionLog.create_session("claimer", log)
    {:ok, _claim} = ActionLog.claim_issue("indexable-inc/index", 4201, claimer, log)

    assert_receive {:mcp_send,
                    %{
                      "method" => "notifications/claude/channel",
                      "params" =>
                        %{
                          "meta" => %{
                            "source" => "issues",
                            "event" => "picked_up",
                            "issue" => "indexable-inc/index#4201"
                          }
                        } = params
                    }},
                   5_000

    assert params["meta"]["session"] == "claimer"
    assert params["content"] =~ "issue picked up: indexable-inc/index#4201"

    # The watermark must swallow both the already-announced claim and the
    # one standing before this watcher booted.
    refute_receive {:mcp_send, %{"params" => %{"meta" => %{"event" => "picked_up"}}}},
                   200
  end
end
