defmodule IxMcp.FleetDigestTest do
  @moduledoc """
  The periodic digest (ENG-11209): suppression when empty, anomaly marking
  against a rolling baseline, and the three mute granularities.

  The digest exists because the raw class it summarises cannot be forwarded:
  the fleet writes ~22,600 journal rows a minute at the median. It has to stay
  silent on an empty window and boring on a normal one, or it becomes the
  fourth ignored red light rather than the thing that makes one visible.
  """

  use ExUnit.Case, async: true

  alias IxMcp.ActionLog
  alias IxMcp.Fleet.Digest
  alias IxMcp.Fleet.Watch

  # The stub answers by SQL shape, the way the real ClickHouse does: the
  # window query groups by level, the baseline query returns one p50 row, the
  # host query groups by node_id.
  defp stub(window_rows, baseline_p50, host_rows) do
    fn sql ->
      cond do
        String.contains?(sql, "quantile(0.5)") -> {:ok, [%{"p50" => baseline_p50}]}
        String.contains?(sql, "GROUP BY node_id") -> {:ok, host_rows}
        true -> {:ok, window_rows}
      end
    end
  end

  defp level_row(level, n) do
    %{
      "level" => level,
      "n" => n,
      "from_ts" => "2026-07-29 12:49:35.916",
      "to_ts" => "2026-07-29 12:50:28.330"
    }
  end

  setup do
    path = Path.join(System.tmp_dir!(), "fleet-digest-#{System.unique_integer([:positive])}.db")
    name = :"fleet_digest_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: path, name: name})
    on_exit(fn -> Enum.each(Path.wildcard(path <> "*"), &File.rm/1) end)
    %{log: name}
  end

  describe "silence still means healthy" do
    test "an empty window produces no digest at all" do
      assert {:ok, nil} = Digest.build(60, stub([], 100.0, []))
    end

    test "an empty window is not rendered as a line of zeroes", %{log: log} do
      me = self()

      assert {:ok, nil} =
               Watch.run_digest(60,
                 query_fun: stub([], 100.0, []),
                 action_log: log,
                 notify: fn d -> send(me, {:digest, d}) end
               )

      refute_received {:digest, _}
    end
  end

  describe "the boring case" do
    test "renders counts by severity and nothing else" do
      assert {:ok, digest} =
               Digest.build(60, stub([level_row("warn", 82), level_row("error", 14)], 265.5, []))

      assert digest.total == 96
      assert digest.counts == %{"warn" => 82, "error" => 14}
      assert digest.anomalies == []
      assert Digest.render(digest) == "82 warnings, 14 errors"
    end

    test "singular counts read as singular" do
      assert {:ok, digest} = Digest.build(60, stub([level_row("error", 1)], 50.0, []))
      assert Digest.render(digest) == "1 error"
    end

    test "carries the window bounds so the drill-in can reproduce it" do
      assert {:ok, digest} = Digest.build(60, stub([level_row("warn", 5)], 10.0, []))
      assert digest.from == "2026-07-29 12:49:35.916"
      assert digest.to == "2026-07-29 12:50:28.330"
    end
  end

  describe "anomaly marking, against a rolling baseline" do
    test "a host far above baseline is called out inline" do
      # Baseline 10/min fleet-wide, so ~10 expected in a 60s window; 500 on one
      # host is 50x and well over the floor.
      assert {:ok, digest} =
               Digest.build(
                 60,
                 stub([level_row("error", 500)], 10.0, [
                   %{"node_id" => "hil-compute-2", "n" => 500}
                 ])
               )

      assert [mark] = digest.anomalies
      assert mark =~ "hil-compute-2"
      assert mark =~ "x baseline"
      assert Digest.render(digest) =~ "500 errors -- hil-compute-2"
    end

    test "a busy but proportionate host is not called out" do
      assert {:ok, digest} =
               Digest.build(
                 60,
                 stub([level_row("warn", 400)], 400.0, [
                   %{"node_id" => "hil-compute-2", "n" => 380}
                 ])
               )

      assert digest.anomalies == []
    end

    test "a small absolute jump is not an anomaly however large the ratio" do
      # 1/min baseline to 6 in the window is 6x, over the ratio, but six lines
      # is not news. Without the floor this fires constantly on quiet hosts.
      assert {:ok, digest} =
               Digest.build(
                 60,
                 stub([level_row("warn", 6)], 1.0, [%{"node_id" => "quiet", "n" => 6}])
               )

      assert digest.anomalies == []
    end

    test "a zero baseline does not divide by zero or mark everything" do
      assert {:ok, digest} =
               Digest.build(
                 60,
                 stub([level_row("warn", 50)], 0.0, [%{"node_id" => "n1", "n" => 50}])
               )

      assert digest.anomalies == []
    end
  end

  describe "unsubscribe covers the digest as well as discrete alerts" do
    test "muting the digest silences the line entirely", %{log: log} do
      assert :ok = ActionLog.mute_fleet_predicate("digest", nil, log)

      assert {:ok, nil} =
               Watch.run_digest(60,
                 query_fun: stub([level_row("warn", 82)], 100.0, []),
                 action_log: log,
                 notify: fn _ -> flunk("muted digest must not announce") end
               )
    end

    test "muting one category drops it from the count but keeps the line", %{log: log} do
      assert :ok = ActionLog.mute_fleet_predicate("digest:warning", nil, log)

      assert {:ok, digest} =
               Watch.run_digest(60,
                 query_fun: stub([level_row("warn", 82), level_row("error", 14)], 100.0, []),
                 action_log: log,
                 notify: fn _ -> :ok end
               )

      assert digest.counts == %{"error" => 14}
      assert digest.total == 14
      assert Digest.render(digest) == "14 errors"
    end

    test "muting the only category present suppresses the line rather than saying zero", %{
      log: log
    } do
      assert :ok = ActionLog.mute_fleet_predicate("digest:warning", nil, log)

      assert {:ok, nil} =
               Watch.run_digest(60,
                 query_fun: stub([level_row("warn", 82)], 100.0, []),
                 action_log: log,
                 notify: fn _ -> flunk("a fully-muted window must say nothing") end
               )
    end

    test "every mute granularity is accepted, and nonsense is refused" do
      assert "digest" in IxMcp.Fleet.mutable()
      assert "digest:warning" in IxMcp.Fleet.mutable()
      assert "ci_oom_success" in IxMcp.Fleet.mutable()

      assert {:error, reason} = IxMcp.Fleet.mute("digest:banana")
      assert reason =~ "unknown predicate"
    end
  end

  describe "the period is adjustable, as the operator asked" do
    test "below the floor is refused rather than silently clamped" do
      assert {:error, reason} = Watch.set_digest_period(5)
      assert reason =~ "at least 10s"
    end

    test "the window length reaches the query" do
      me = self()

      spy = fn sql ->
        send(me, {:sql, sql})
        {:ok, []}
      end

      Digest.build(300, spy)
      assert_received {:sql, sql}
      assert sql =~ "INTERVAL 300 SECOND"
    end
  end

  describe "a failed read is not an empty fleet" do
    test "build/2 propagates the error rather than reporting nothing happened" do
      assert {:error, reason} = Digest.build(60, fn _ -> {:error, "no route to host"} end)
      assert reason =~ "no route to host"
    end

    test "a baseline read failing does not silently produce a baseline of zero" do
      partial = fn sql ->
        if String.contains?(sql, "quantile(0.5)"),
          do: {:error, "baseline unreadable"},
          else: {:ok, [%{"level" => "warn", "n" => 5, "from_ts" => "a", "to_ts" => "b"}]}
      end

      assert {:error, reason} = Digest.build(60, partial)
      assert reason =~ "baseline unreadable"
    end
  end
end
