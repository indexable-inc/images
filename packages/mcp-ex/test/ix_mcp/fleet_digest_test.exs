defmodule IxMcp.FleetDigestTest do
  @moduledoc """
  The two emission rates (ENG-11209): an hourly heartbeat that makes the
  baseline visible, and an immediate anomaly line for a minute that is out of
  band.

  The cadence is the whole design. A 60-second unconditional digest emits on
  87.1% of minutes -- ~1,250 lines a day -- and a ratio-against-rolling-median
  anomaly rule fires 605.7 times a day on real per-host data. Both were
  measured and both were rejected; the tests below pin the shapes that
  replaced them.
  """

  use ExUnit.Case, async: true

  alias IxMcp.ActionLog
  alias IxMcp.Fleet.Digest
  alias IxMcp.Fleet.Watch
  alias IxMcp.MCP.Notifier

  # The stub answers by SQL shape, as the real ClickHouse does: the window query
  # groups by level, the threshold query takes a quantile, the anomaly query
  # groups by node_id over one minute.
  defp stub(opts) do
    fn sql ->
      cond do
        String.contains?(sql, "quantile(") -> {:ok, [%{"threshold" => opts[:threshold]}]}
        String.contains?(sql, "GROUP BY node_id") -> {:ok, opts[:hosts] || []}
        true -> {:ok, opts[:window] || []}
      end
    end
  end

  defp level_row(level, n) do
    %{
      "level" => level,
      "n" => n,
      "from_ts" => "2026-07-29 12:00:00.000",
      "to_ts" => "2026-07-29 12:59:59.000"
    }
  end

  defp host_row(node, n),
    do: %{"node_id" => node, "n" => n, "minute" => "2026-07-29 12:49:00"}

  setup do
    path = Path.join(System.tmp_dir!(), "fleet-digest-#{System.unique_integer([:positive])}.db")
    name = :"fleet_digest_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: path, name: name})
    on_exit(fn -> Enum.each(Path.wildcard(path <> "*"), &File.rm/1) end)
    %{log: name}
  end

  describe "the hourly heartbeat" do
    test "renders counts by severity and names its window" do
      assert {:ok, digest} =
               Digest.build(
                 3_600,
                 stub(window: [level_row("warn", 4070), level_row("error", 643)])
               )

      assert digest.total == 4713
      assert Digest.render(digest) == "4070 warnings, 643 errors in the last 1h"
    end

    test "an empty window says nothing at all", %{log: log} do
      assert {:ok, nil} = Digest.build(3_600, stub(window: []))

      assert {:ok, nil} =
               Watch.run_heartbeat(3_600,
                 query_fun: stub(window: []),
                 action_log: log,
                 notify: fn _ -> flunk("an empty window must not announce") end
               )
    end

    test "carries window bounds so the drill-in can reproduce it" do
      assert {:ok, digest} = Digest.build(3_600, stub(window: [level_row("warn", 5)]))
      assert digest.from == "2026-07-29 12:00:00.000"
      assert digest.to == "2026-07-29 12:59:59.000"
    end

    test "a per-minute heartbeat is refused, because that is the rejected design" do
      assert {:error, reason} = Watch.set_heartbeat_period(30)
      assert reason =~ "at least 60s"
      # The refusal explains itself with the number, so the next person does not
      # have to re-derive why hourly was chosen.
      assert reason =~ "1,250 lines/day"
    end
  end

  describe "the anomaly line, thresholded by quantile rather than ratio" do
    test "a minute over this hour's threshold fires and names the culprits" do
      assert {:ok, anomaly} =
               Digest.check_anomaly(
                 3_121.0,
                 stub(hosts: [host_row("hil-compute-2", 9_000), host_row("vin-compute-1", 400)])
               )

      assert anomaly.count == 9_400
      rendered = Digest.render_anomaly(anomaly)
      assert rendered =~ "9400 notable in one minute"
      assert rendered =~ "threshold of 3121"
      assert rendered =~ "hil-compute-2 9000"
    end

    test "a normal minute is silent even though it is far above the median" do
      # 400 is ~12x the median minute of 33 and still nowhere near the p99.5 for
      # this hour. Under the rejected ratio rule this would have fired; that is
      # the whole reason the rule was rejected.
      assert {:ok, nil} = Digest.check_anomaly(3_121.0, stub(hosts: [host_row("n1", 400)]))
    end

    test "a small absolute count cannot fire however low the threshold" do
      assert {:ok, nil} = Digest.check_anomaly(1.0, stub(hosts: [host_row("quiet", 6)]))
    end

    test "a zero threshold does not mark everything" do
      assert {:ok, nil} = Digest.check_anomaly(0.0, stub(hosts: [host_row("n1", 5_000)]))
    end

    test "an empty minute is not an anomaly" do
      assert {:ok, nil} = Digest.check_anomaly(100.0, stub(hosts: []))
    end

    test "the threshold is cached per clock hour, not recomputed every minute", %{log: log} do
      me = self()

      counting = fn sql ->
        if String.contains?(sql, "quantile("), do: send(me, :threshold_query)

        if String.contains?(sql, "quantile("),
          do: {:ok, [%{"threshold" => 500.0}]},
          else: {:ok, []}
      end

      opts = [query_fun: counting, action_log: log, notify: fn _ -> :ok end, hour: 12]

      {_r1, cached} = Watch.run_anomaly(nil, opts)
      assert_received :threshold_query

      {_r2, cached} = Watch.run_anomaly(cached, opts)
      refute_received :threshold_query

      # A new clock hour recomputes it.
      {_r3, _} = Watch.run_anomaly(cached, Keyword.put(opts, :hour, 13))
      assert_received :threshold_query
    end

    test "a threshold that cannot be read is dropped rather than kept stale", %{log: log} do
      failing = fn sql ->
        if String.contains?(sql, "quantile("), do: {:error, "unreachable"}, else: {:ok, []}
      end

      assert {{:error, reason}, nil} =
               Watch.run_anomaly({11, 500.0},
                 query_fun: failing,
                 action_log: log,
                 notify: fn _ -> :ok end,
                 hour: 12
               )

      assert reason =~ "unreachable"
    end
  end

  describe "unsubscribe covers both rates" do
    test "muting the heartbeat leaves the anomaly line alive", %{log: log} do
      assert :ok = ActionLog.mute_fleet_predicate("heartbeat", nil, log)

      assert {:ok, nil} =
               Watch.run_heartbeat(3_600,
                 query_fun: stub(window: [level_row("warn", 82)]),
                 action_log: log,
                 notify: fn _ -> flunk("muted heartbeat must not announce") end
               )

      me = self()

      {{:ok, anomaly}, _} =
        Watch.run_anomaly(nil,
          query_fun: stub(threshold: 100.0, hosts: [host_row("n1", 9_000)]),
          action_log: log,
          notify: fn a -> send(me, {:anomaly, a}) end,
          hour: 12
        )

      assert anomaly.count == 9_000
      assert_received {:anomaly, _}
    end

    test "muting the anomaly leaves the heartbeat alive", %{log: log} do
      assert :ok = ActionLog.mute_fleet_predicate("anomaly", nil, log)

      assert {{:ok, nil}, _} =
               Watch.run_anomaly(nil,
                 query_fun: stub(threshold: 100.0, hosts: [host_row("n1", 9_000)]),
                 action_log: log,
                 notify: fn _ -> flunk("muted anomaly must not announce") end,
                 hour: 12
               )

      assert {:ok, digest} =
               Watch.run_heartbeat(3_600,
                 query_fun: stub(window: [level_row("warn", 82)]),
                 action_log: log,
                 notify: fn _ -> :ok end
               )

      assert digest.total == 82
    end

    test "muting \"digest\" silences both rates", %{log: log} do
      assert :ok = ActionLog.mute_fleet_predicate("digest", nil, log)

      assert {:ok, nil} =
               Watch.run_heartbeat(3_600,
                 query_fun: stub(window: [level_row("warn", 82)]),
                 action_log: log,
                 notify: fn _ -> flunk("digest mute must cover the heartbeat") end
               )

      assert {{:ok, nil}, _} =
               Watch.run_anomaly(nil,
                 query_fun: stub(threshold: 100.0, hosts: [host_row("n1", 9_000)]),
                 action_log: log,
                 notify: fn _ -> flunk("digest mute must cover the anomaly") end,
                 hour: 12
               )
    end

    test "muting one category drops it from the count but keeps the line", %{log: log} do
      assert :ok = ActionLog.mute_fleet_predicate("digest:warning", nil, log)

      assert {:ok, digest} =
               Watch.run_heartbeat(3_600,
                 query_fun: stub(window: [level_row("warn", 82), level_row("error", 14)]),
                 action_log: log,
                 notify: fn _ -> :ok end
               )

      assert digest.counts == %{"error" => 14}
      assert Digest.render(digest) == "14 errors in the last 1h"
    end

    test "muting the only category present suppresses rather than saying zero", %{log: log} do
      assert :ok = ActionLog.mute_fleet_predicate("digest:warning", nil, log)

      assert {:ok, nil} =
               Watch.run_heartbeat(3_600,
                 query_fun: stub(window: [level_row("warn", 82)]),
                 action_log: log,
                 notify: fn _ -> flunk("a fully-muted window must say nothing") end
               )
    end

    test "every mute shape is accepted and nonsense is refused" do
      for id <- ~w(heartbeat anomaly digest digest:warning observability_blind) do
        assert id in IxMcp.Fleet.mutable(), "#{id} must be mutable"
      end

      assert {:error, reason} = IxMcp.Fleet.mute("digest:banana")
      assert reason =~ "unknown predicate"
    end
  end

  describe "a failed read is not an empty fleet" do
    test "the heartbeat propagates the error rather than reporting nothing happened" do
      assert {:error, reason} = Digest.build(3_600, fn _ -> {:error, "no route to host"} end)
      assert reason =~ "no route to host"
    end

    test "the anomaly check propagates its error too" do
      assert {:error, reason} = Digest.check_anomaly(100.0, fn _ -> {:error, "no route"} end)
      assert reason =~ "no route"
    end
  end

  describe "meta values are enforced as strings, not merely documented" do
    test "a non-string meta value raises at the producer rather than vanishing" do
      # The client parses meta as string-to-string and drops the whole event on
      # anything else, with nothing reaching the sender. That is a silent-failure
      # mode inside a system built to end silent failures, so the guard has to
      # be watched failing at least once.
      assert_raise ArgumentError, ~r/meta values must be strings/, fn ->
        Notifier.channel("fleet: 5 errors", %{"total" => 5})
      end
    end

    test "the string form is accepted" do
      assert :ok = Notifier.channel("fleet: 5 errors", %{"total" => "5"})
    end
  end
end
