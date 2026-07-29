defmodule IxMcp.FleetAlertsTest do
  @moduledoc """
  The three break-tests for fleet notifications (ENG-11209). Each one forces
  the condition and watches the mechanism fail or fire, because a guard nobody
  has watched fail is not a guard -- and this feature was built after finding
  three separate alerting mechanisms on this fleet that could never have fired.
  """

  use ExUnit.Case, async: true

  alias IxMcp.ActionLog
  alias IxMcp.Fleet.Alerts
  alias IxMcp.Fleet.Topology
  alias IxMcp.Fleet.Watch
  alias IxMcp.MCP.Tools

  # One XFS corruption line, shaped exactly like the rows ClickHouse returned
  # from logs.journald_logs on 2026-07-29 (the real, unnoticed incident that
  # motivated the predicate; ENG-11210).
  @xfs_row %{
    "node_id" => "hil-compute-2",
    "level" => "alert",
    "message" => "XFS (dm-2): Metadata CRC error detected at xfs_agf_read_verify+0x7e/0x110 [xfs]"
  }

  @oom_row %{
    "node_id" => "vin-compute-1",
    "comm" => "nix-eval-jobs",
    "hour" => "2026-07-26 03:00:00",
    "kills" => 3,
    "peak_rss_gb" => 1.69
  }

  # One CI job that absorbed kernel OOM kills and reported green anyway, shaped
  # like the real rows: on 2026-07-26 three `push (shard N)` jobs on
  # vin-compute-1 did exactly this and all reported success.
  @ci_oom_row %{
    "node_id" => "vin-compute-1",
    "comm" => "nix-eval-jobs",
    "kills" => 9,
    "job_names" => ["push (shard 0)", "push (shard 1)", "push (shard 2)"],
    "units" => ["ix-ci-job-index-30185401611-89749023189.service"],
    "conclusions" => ["success"]
  }

  # A stub standing in for ClickHouse: answers the kernel_storage query with
  # rows and everything else with none, so exactly one predicate fires.
  defp only_kernel_storage(sql) do
    if String.contains?(sql, "journald_logs"), do: {:ok, [@xfs_row]}, else: {:ok, []}
  end

  # Both OOM predicates read logs.oom_kills, so the table name does not
  # identify either. The burst query is the one that buckets by hour; the CI
  # query is the one that joins kpi.ci_jobs.
  defp only_oom(sql) do
    if String.contains?(sql, "toStartOfHour"), do: {:ok, [@oom_row]}, else: {:ok, []}
  end

  defp only_ci_oom(sql) do
    if String.contains?(sql, "kpi.ci_jobs"), do: {:ok, [@ci_oom_row]}, else: {:ok, []}
  end

  defp quiet_fleet(_sql), do: {:ok, []}

  defp unreachable(_sql),
    do: {:error, "hil-compute-2: ssh: connect to host port 22: No route to host"}

  setup do
    # A real file, not :memory: -- the mute test has to survive the log process
    # dying and restarting, which is what a reconnect looks like from here, and
    # an in-memory database cannot demonstrate that.
    path = Path.join(System.tmp_dir!(), "fleet-alerts-#{System.unique_integer([:positive])}.db")
    name = :"fleet_alerts_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: path, name: name})
    on_exit(fn -> Enum.each(Path.wildcard(path <> "*"), &File.rm/1) end)
    %{log: name, path: path}
  end

  defp collect(hits, target), do: send(target, {:announced, hits})

  describe "break-test 1: a forced condition arrives with the intended content" do
    test "the XFS corruption predicate fires and names the host and the fault", %{log: log} do
      me = self()

      result =
        Watch.run_poll("warning",
          query_fun: &only_kernel_storage/1,
          action_log: log,
          notify: &collect(&1, me),
          deliverable: true
        )

      assert [hit] = result.announced
      assert hit.predicate == "kernel_storage"
      assert hit.level == "critical"
      assert hit.summary =~ "hil-compute-2"
      assert hit.summary =~ "Metadata CRC error"
      assert result.errors == []

      # The notification actually left, carrying the same hit.
      assert_received {:announced, [^hit]}
    end

    test "a CI job that absorbed OOM kills and reported success is named loudly", %{log: log} do
      result =
        Watch.run_poll("warning",
          query_fun: &only_ci_oom/1,
          action_log: log,
          notify: fn _ -> :ok end,
          deliverable: true
        )

      assert [hit] = result.announced
      assert hit.predicate == "ci_oom_success"
      assert hit.level == "critical"
      assert hit.summary =~ "reported SUCCESS"
      assert hit.summary =~ "push (shard 0)"
      # The attribution caveat travels with the alert rather than living only in
      # a moduledoc, because the reader has to know 9 is distinct kills and the
      # job list is candidates.
      assert hit.summary =~ "attribution is by unit lifetime"
    end

    test "a quiet fleet announces nothing at all", %{log: log} do
      me = self()

      result =
        Watch.run_poll("warning",
          query_fun: &quiet_fleet/1,
          action_log: log,
          notify: &collect(&1, me),
          deliverable: true
        )

      assert result == %{announced: [], suppressed: 0, errors: []}
      refute_received {:announced, _}
    end

    test "the same condition standing across polls announces exactly once", %{log: log} do
      opts = [
        query_fun: &only_kernel_storage/1,
        action_log: log,
        notify: fn _ -> :ok end,
        deliverable: true
      ]

      first = Watch.run_poll("warning", opts)
      second = Watch.run_poll("warning", opts)
      third = Watch.run_poll("warning", opts)

      assert length(first.announced) == 1
      assert second.announced == []
      assert second.suppressed == 1
      assert third.announced == []

      # ...and it is visible as standing rather than forgotten.
      assert [%{predicate: "kernel_storage"}] = ActionLog.fleet_alerts_seen(log)
    end

    test "a genuinely new event announces again even while an old one stands", %{log: log} do
      opts = [action_log: log, notify: fn _ -> :ok end, deliverable: true]

      assert length(Watch.run_poll("warning", [query_fun: &only_oom/1] ++ opts).announced) == 1

      # Same predicate, new hour bucket: a fresh burst, so it is news.
      next_hour = fn sql ->
        if String.contains?(sql, "toStartOfHour"),
          do: {:ok, [%{@oom_row | "hour" => "2026-07-26 04:00:00"}]},
          else: {:ok, []}
      end

      assert length(Watch.run_poll("warning", [query_fun: next_hour] ++ opts).announced) == 1
    end
  end

  describe "break-test 2: muting silences it, and the mute survives a reconnect" do
    test "mute stops the notification, unmute restores it", %{log: log} do
      opts = [
        query_fun: &only_kernel_storage/1,
        action_log: log,
        notify: fn _ -> :ok end,
        deliverable: true
      ]

      assert :ok = ActionLog.mute_fleet_predicate("kernel_storage", "testing", log)
      assert Watch.run_poll("warning", opts).announced == []

      assert :ok = ActionLog.unmute_fleet_predicate("kernel_storage", log)
      assert length(Watch.run_poll("warning", opts).announced) == 1
    end

    test "the mute survives the log process dying and coming back", %{log: log, path: path} do
      assert :ok = ActionLog.mute_fleet_predicate("oom_burst", "too noisy", log)
      assert [%{id: "oom_burst", reason: "too noisy"}] = ActionLog.fleet_mutes(log)

      # A reconnect, from the durability point of view: the process holding the
      # state goes away and a new one opens the same file. An in-memory mute
      # would be gone here, which is exactly the failure this asserts against.
      stop_supervised!(ActionLog)
      reborn = :"fleet_alerts_reborn_#{System.unique_integer([:positive])}"
      start_supervised!({ActionLog, path: path, name: reborn})

      assert [%{id: "oom_burst", reason: "too noisy"}] = ActionLog.fleet_mutes(reborn)

      # And it is still honoured, not merely still recorded.
      result =
        Watch.run_poll("warning",
          query_fun: &only_oom/1,
          action_log: reborn,
          notify: fn _ -> :ok end
        )

      assert result.announced == []
    end

    test "an unknown predicate id is refused rather than silently accepted" do
      assert {:error, reason} = IxMcp.Fleet.mute("kernel-storage")
      assert reason =~ "unknown predicate"
      assert reason =~ "kernel_storage"
    end

    test "the level floor is the coarse unsubscribe", %{log: log} do
      # kernel_storage is `critical`, oom_burst is `warning`.
      assert Watch.at_or_above?("critical", "warning")
      refute Watch.at_or_above?("warning", "critical")

      result =
        Watch.run_poll("critical",
          query_fun: &only_oom/1,
          action_log: log,
          notify: fn _ -> :ok end,
          deliverable: true
        )

      assert result.announced == [], "a warning must not survive a critical floor"
    end
  end

  describe "break-test 3: an unreachable source reports blindness, never health" do
    test "a failed read is an error and an alert, not an empty result", %{log: log} do
      me = self()

      result =
        Watch.run_poll("warning",
          query_fun: &unreachable/1,
          action_log: log,
          notify: &collect(&1, me),
          deliverable: true
        )

      refute result.errors == []
      assert Enum.all?(result.errors, &(&1 =~ "No route to host"))

      assert [hit] = result.announced
      assert hit.predicate == "observability_blind"

      # The wording matters as much as the firing: the whole point is that this
      # cannot be mistaken for a quiet fleet.
      assert hit.summary =~ "NOT a report of a healthy fleet"
      assert hit.summary =~ "No route to host"
      assert_received {:announced, [^hit]}
    end

    test "evaluate/2 surfaces per-predicate errors instead of empty lists" do
      outcomes = Alerts.evaluate([], &unreachable/1)

      assert {:error, _} = outcomes["kernel_storage"]
      assert {:error, _} = outcomes["oom_burst"]
      assert {:ok, [_blind]} = outcomes["observability_blind"]
    end

    test "a persistent outage announces once, not once per poll", %{log: log} do
      opts = [
        query_fun: &unreachable/1,
        action_log: log,
        notify: fn _ -> :ok end,
        deliverable: true
      ]

      assert length(Watch.run_poll("warning", opts).announced) == 1
      assert Watch.run_poll("warning", opts).announced == []
    end

    test "blindness itself can be muted", %{log: log} do
      assert :ok = ActionLog.mute_fleet_predicate("observability_blind", nil, log)

      result =
        Watch.run_poll("warning",
          query_fun: &unreachable/1,
          action_log: log,
          notify: fn _ -> :ok end,
          deliverable: true
        )

      assert result.announced == []
      # Muted from the channel, but still reported to a caller who asked.
      refute result.errors == []
    end
  end

  describe "topology" do
    test "distribution being down is reported as unknown, not as zero reachable" do
      rendered =
        Topology.render(%{
          configured: [:"beamd@a.example", :"beamd@b.example"],
          nodes: [{:"beamd@a.example", :unknown}, {:"beamd@b.example", :unknown}],
          distribution: {:error, :nodistribution},
          local: :nonode@nohost
        })

      assert rendered =~ "liveness UNKNOWN"
      assert rendered =~ "2 node(s) configured"
      refute rendered =~ "0 of 2"
      assert rendered =~ "a, b"
    end

    test "a working mesh reports which hosts are up and which are not" do
      rendered =
        Topology.render(%{
          configured: [:"beamd@a.example", :"beamd@b.example"],
          nodes: [{:"beamd@a.example", :up}, {:"beamd@b.example", :down}],
          distribution: :ok,
          local: :mcp@here
        })

      assert rendered =~ "1 of 2 node(s) reachable"
      assert rendered =~ "Up: a"
      assert rendered =~ "Unreachable: b"
    end

    test "no configured nodes says so plainly" do
      rendered =
        Topology.render(%{configured: [], nodes: [], distribution: :ok, local: :nonode@nohost})

      assert rendered =~ "no nodes configured"
      assert rendered =~ "IX_BEAM_NODES"
    end
  end

  describe "the unsubscribe is discoverable where the operator will see it" do
    test "the exec tool description names the mute and the ids" do
      [%{"description" => description}] = Tools.list()

      assert description =~ "UNSUBSCRIBE"
      assert description =~ "Fleet.mute"
      assert description =~ "logging/setLevel"

      for id <- Alerts.ids() do
        assert description =~ id, "tool description must name predicate #{id}"
      end
    end

    test "every catalogued predicate has a level" do
      for id <- Alerts.ids() do
        assert Alerts.level(id) in Watch.levels()
      end
    end
  end

  describe "regressions from the adversarial review" do
    # The first draft excluded dm-* alongside loop/nbd, which made the predicate
    # match ZERO rows in 30 days -- including the very ENG-11210 lines its
    # measured-rate table cited, which were on dm-2. Verified against production:
    # shipped SQL 0 hits, loop/nbd-only 3 hits. This asserts on the SQL text
    # because the bug lived in the SQL and every other test stubs past it.
    test "the storage predicate excludes images but not device-mapper" do
      sql = captured_sql("Unmount and run xfs_repair")

      assert sql =~ "(loop|nbd)", "loop and nbd must still be excluded"

      refute sql =~ "(loop|nbd|dm-)",
             "dm-* is LVM/LUKS host storage; excluding it made the predicate dead"
    end

    # The no-recovery suppression used a different regex on each side, and the
    # outer one returns "" for three of the four signatures, so (node,'') paired
    # with (node,'') and any process logging the phrase silenced every storage
    # alert on that host. ix-system-clickhouse logs it at info level whenever
    # this file's own SQL comes back in an exception.
    test "the deliberate-mount suppression cannot become a host-wide wildcard" do
      sql = captured_sql("Unmount and run xfs_repair")

      assert sql =~ "AND systemd_unit = ''", "the CTE must require a kernel source"
      assert sql =~ ~r/deliberate AS \(.*?!= ''/s, "the CTE must reject an empty device"

      assert sql =~ ~r/extract\(message, '.+?'\) != ''\n\s+AND \(node_id/,
             "an empty extraction must never match an empty CTE row"
    end

    defp captured_sql(marker) do
      {:ok, agent} = Agent.start_link(fn -> [] end)

      Alerts.evaluate([], fn sql ->
        Agent.update(agent, &[sql | &1])
        {:ok, []}
      end)

      agent |> Agent.get(& &1) |> Enum.find(&String.contains?(&1, marker))
    end

    test "a level-filtered hit is still recorded, so lowering the floor cannot replay it",
         %{log: log} do
      opts = [query_fun: &only_oom/1, action_log: log, notify: fn _ -> :ok end, deliverable: true]

      # oom_burst is a warning; a critical floor drops it from delivery.
      assert Watch.run_poll("critical", opts).announced == []

      # Lowering the floor must NOT replay it: it was recorded as seen.
      assert Watch.run_poll("warning", opts).announced == []
    end

    test "nothing is recorded as seen when no transport can receive it", %{log: log} do
      opts = [
        query_fun: &only_kernel_storage/1,
        action_log: log,
        notify: fn _ -> :ok end,
        deliverable: true
      ]

      assert Watch.run_poll("warning", Keyword.put(opts, :deliverable, false)).announced == []
      assert ActionLog.fleet_alerts_seen(log) == [], "an undelivered alert must stay unrecorded"

      # ...so it still announces once somebody is listening. Without this the
      # fleet path buries any fault raised between sessions: unlike jobs, it has
      # no outbox replay.
      assert length(Watch.run_poll("warning", Keyword.put(opts, :deliverable, true)).announced) ==
               1
    end

    test "blindness is exempt from the level floor", %{log: log} do
      # Raising the floor is the natural response to noise, and it must not be
      # the thing that hides "I cannot see the fleet".
      result =
        Watch.run_poll("emergency",
          query_fun: &unreachable/1,
          action_log: log,
          notify: fn _ -> :ok end,
          deliverable: true
        )

      assert [hit] = result.announced
      assert hit.predicate == "observability_blind"
    end

    test "a recovered read re-arms blindness, so a second outage is not silent", %{log: log} do
      opts = [action_log: log, notify: fn _ -> :ok end, deliverable: true]

      assert length(Watch.run_poll("warning", [query_fun: &unreachable/1] ++ opts).announced) == 1
      assert Watch.run_poll("warning", [query_fun: &unreachable/1] ++ opts).announced == []

      # A successful poll clears the fingerprint...
      Watch.run_poll("warning", [query_fun: &quiet_fleet/1] ++ opts)

      # ...so the next outage announces rather than being deduped forever.
      assert length(Watch.run_poll("warning", [query_fun: &unreachable/1] ++ opts).announced) == 1
    end
  end

  describe "the disposition table, which is the specification" do
    # Three bugs came from collapsing "deliberately filtered" and "could not be
    # attempted" into one "was not announced". These pin all three rows, so a
    # future edit that re-collapses them fails here rather than in production.

    test "delivered: consumes the fingerprint", %{log: log} do
      opts = [
        query_fun: &only_kernel_storage/1,
        action_log: log,
        notify: fn _ -> :ok end,
        deliverable: true
      ]

      assert length(Watch.run_poll("warning", opts).announced) == 1
      assert [%{predicate: "kernel_storage"}] = ActionLog.fleet_alerts_seen(log)
      # ...and does not announce twice.
      assert Watch.run_poll("warning", opts).announced == []
    end

    test "filtered by the level floor: consumes, so lowering the floor cannot replay it",
         %{log: log} do
      opts = [query_fun: &only_oom/1, action_log: log, notify: fn _ -> :ok end, deliverable: true]

      # oom_burst is a warning; a critical floor drops it.
      assert Watch.run_poll("critical", opts).announced == []
      assert [%{predicate: "oom_burst"}] = ActionLog.fleet_alerts_seen(log)

      assert Watch.run_poll("warning", opts).announced == [],
             "a deliberately filtered hit must not come back when the floor drops"
    end

    test "could not be attempted: does NOT consume, so it still announces later",
         %{log: log} do
      opts = [query_fun: &only_kernel_storage/1, action_log: log, notify: fn _ -> :ok end]

      assert Watch.run_poll("warning", Keyword.put(opts, :deliverable, false)).announced == []

      assert ActionLog.fleet_alerts_seen(log) == [],
             "nothing was attempted, so nothing may be recorded"

      assert length(Watch.run_poll("warning", Keyword.put(opts, :deliverable, true)).announced) ==
               1
    end

    test "the three rows are genuinely different, not two spellings of one", %{log: log} do
      # Same hit, same absence of an announcement, opposite ledger effects.
      filtered = [
        query_fun: &only_oom/1,
        action_log: log,
        notify: fn _ -> :ok end,
        deliverable: true
      ]

      Watch.run_poll("critical", filtered)
      after_filter = length(ActionLog.fleet_alerts_seen(log))

      ActionLog.forget_fleet_alerts(:all, log)

      undeliverable = Keyword.put(filtered, :deliverable, false)
      Watch.run_poll("warning", undeliverable)
      after_defer = length(ActionLog.fleet_alerts_seen(log))

      assert after_filter == 1, "a filtered hit is consumed"
      assert after_defer == 0, "an unattempted hit is not"
    end
  end
end
