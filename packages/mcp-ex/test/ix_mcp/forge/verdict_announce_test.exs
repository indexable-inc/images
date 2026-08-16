defmodule IxMcp.Forge.VerdictAnnounceTest do
  use ExUnit.Case, async: false

  alias IxMcp.Forge.VerdictAnnounce
  alias IxMcp.MCP.Notifier

  # A fixture label, not "forge": IX_MCP_STDIO=1 reaches `mix test` from a
  # kernel-launched shell, so a real feed can be live in this VM and an
  # assertion matching its label could pass on a real verdict.
  @source "fixture"
  @pass %{
    id: "1f2e3d4c5b6a-1786546496920",
    verdict: :pass,
    change_id: "0a1b2c3d4e5f",
    commit_id: "1f2e3d4c5b6a",
    target: "main",
    duration_ms: 309_000,
    failed_stages: [],
    tolerated: [],
    log: nil
  }
  @fail %{
    @pass
    | id: "9d8c7b6a5f4e-1786543762957",
      verdict: :fail,
      change_id: "f9e8d7c6b5a4",
      commit_id: "9d8c7b6a5f4e",
      duration_ms: 388_000,
      failed_stages: ["incr"],
      tolerated: ["fixture-tolerated-check"],
      log: "/fixture/logs/gate.log"
  }

  setup do
    Notifier.register(self())
    _state = :sys.get_state(Notifier)
    :ok
  end

  defp await_channel do
    assert_receive {:mcp_send,
                    %{
                      "method" => "notifications/claude/channel",
                      "params" => %{"meta" => %{"source" => @source}} = params
                    }},
                   2_000

    params
  end

  test "a pass is one line naming the bookmark, both prefixes, and the run duration" do
    VerdictAnnounce.announce(@source, @pass)

    assert %{"content" => content, "meta" => meta} = await_channel()
    assert content == "fixture CI PASS main 1f2e3d4c5b6a (change 0a1b2c3d4e5f) in 5m9s"
    assert meta["verdict"] == "pass"
    assert meta["commit"] == "1f2e3d4c5b6a"
    assert meta["change"] == "0a1b2c3d4e5f"
    # The untruncated run id rides in meta, because the line's prefixes are
    # for recognition and this is the handle for a follow-up read.
    assert meta["id"] == "1f2e3d4c5b6a-1786546496920"

    # The client parses meta as string-to-string and drops the whole event on
    # anything else, so this is the wire contract, not a style preference.
    assert Enum.all?(meta, fn {key, value} -> is_binary(key) and is_binary(value) end)
  end

  test "a fail names the failing stages, the already-red set, and the log" do
    VerdictAnnounce.announce(@source, @fail)

    assert %{"content" => content, "meta" => meta} = await_channel()

    assert content ==
             "fixture CI FAIL main 9d8c7b6a5f4e (change f9e8d7c6b5a4) in 6m28s; " <>
               "failed: incr; already red on target: fixture-tolerated-check; " <>
               "log: /fixture/logs/gate.log"

    assert meta["verdict"] == "fail"
  end

  # The garnish must never be able to block the signal: a gate whose output
  # changed shape still has to produce a red verdict a reader can act on.
  test "a fail whose detail could not be parsed is still a complete line" do
    VerdictAnnounce.announce(@source, %{@fail | failed_stages: [], tolerated: [], log: nil})

    assert %{"content" => content} = await_channel()
    assert content == "fixture CI FAIL main 9d8c7b6a5f4e (change f9e8d7c6b5a4) in 6m28s"
  end

  test "an unknown duration says so rather than rendering as instant" do
    VerdictAnnounce.announce(@source, %{@pass | duration_ms: nil})

    assert %{"content" => content} = await_channel()
    assert content =~ "in unknown time"
  end

  test "overflow is announced without naming the runs it dropped" do
    VerdictAnnounce.announce_overflow(@source, 20)

    assert %{"content" => content, "meta" => meta} = await_channel()
    assert content =~ "more CI runs reached a verdict than this sweep's limit of 20"
    assert meta["overflow"] == "true"
  end

  test "durations read as a human would say them" do
    assert VerdictAnnounce.duration(0) == "0s"
    assert VerdictAnnounce.duration(48_000) == "48s"
    assert VerdictAnnounce.duration(60_000) == "1m0s"
    assert VerdictAnnounce.duration(309_000) == "5m9s"
    assert VerdictAnnounce.duration(3_600_000) == "1h0m"
    assert VerdictAnnounce.duration(3_840_000) == "1h4m"
    assert VerdictAnnounce.duration(nil) == "unknown time"
  end
end
