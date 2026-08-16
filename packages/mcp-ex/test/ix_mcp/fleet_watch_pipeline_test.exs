defmodule IxMcp.FleetWatchPipelineTest do
  @moduledoc """
  The announce pipeline against a synthetic catalog: dedup, mutes, the level
  floor, and blindness. The REAL catalog's break-tests (forced XFS rows, CI
  OOM rows) live with the catalog, in packages/fleet-policy, which drives
  this same pipeline through a test-only path dep. This file owns what is
  mechanism: everything that must hold whatever the policy says.
  """

  use ExUnit.Case, async: true

  alias FleetMesh.Condition
  alias IxMcp.ActionLog
  alias IxMcp.Fleet.WarningsWatch
  alias IxMcp.Fleet.Watch

  defp condition(id, severity, check) do
    Condition.new(
      id: id,
      severity: severity,
      description: "synthetic #{id}",
      interval_ms: 60_000,
      check: check
    )
  end

  defp firing(id, level, fingerprint, summary) do
    hit = %{
      predicate: Atom.to_string(id),
      level: level,
      fingerprint: fingerprint,
      summary: summary
    }

    condition(id, String.to_existing_atom(level), fn -> {:red, [hit]} end)
  end

  defp quiet(id), do: condition(id, :warning, fn -> :green end)

  defp unreachable(id) do
    condition(id, :warning, fn ->
      {:unknown, "hil-compute-2: ssh: connect to host port 22: No route to host"}
    end)
  end

  setup do
    path = Path.join(System.tmp_dir!(), "fleet-pipe-#{System.unique_integer([:positive])}.db")
    name = :"fleet_pipe_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: path, name: name})
    on_exit(fn -> Enum.each(Path.wildcard(path <> "*"), &File.rm/1) end)
    %{log: name}
  end

  defp collect(hits, target), do: send(target, {:announced, hits})

  test "a firing condition is announced with its own content", %{log: log} do
    me = self()

    result =
      Watch.run_poll("warning",
        conditions: [firing(:disk_gone, "critical", "disk_gone:h1", "h1 lost a disk")],
        action_log: log,
        notify: &collect(&1, me),
        deliverable: true
      )

    assert [hit] = result.announced
    assert hit.predicate == "disk_gone"
    assert hit.summary == "h1 lost a disk"
    assert result.errors == []
    assert_received {:announced, [^hit]}
  end

  test "a quiet catalog announces nothing", %{log: log} do
    me = self()

    result =
      Watch.run_poll("warning",
        conditions: [quiet(:calm)],
        action_log: log,
        notify: &collect(&1, me),
        deliverable: true
      )

    assert result == %{announced: [], suppressed: 0, errors: []}
    refute_received {:announced, _}
  end

  test "a standing condition announces once; a new fingerprint is news", %{log: log} do
    opts = [action_log: log, notify: fn _ -> :ok end, deliverable: true]
    same = [conditions: [firing(:burst, "warning", "burst:h1:03", "burst at 03")]]

    assert [_] = Watch.run_poll("warning", same ++ opts).announced
    second = Watch.run_poll("warning", same ++ opts)
    assert second.announced == []
    assert second.suppressed == 1

    fresh = [conditions: [firing(:burst, "warning", "burst:h1:04", "burst at 04")]]
    assert [_] = Watch.run_poll("warning", fresh ++ opts).announced
  end

  test "a mute silences evaluation and announcement, unmute restores", %{log: log} do
    opts = [
      conditions: [firing(:noisy, "critical", "noisy:1", "noise")],
      action_log: log,
      notify: fn _ -> :ok end,
      deliverable: true
    ]

    assert :ok = ActionLog.mute_fleet_predicate("noisy", "testing", log)
    assert Watch.run_poll("warning", opts).announced == []

    assert :ok = ActionLog.unmute_fleet_predicate("noisy", log)
    assert [_] = Watch.run_poll("warning", opts).announced
  end

  test "the level floor consumes below-floor hits", %{log: log} do
    result =
      Watch.run_poll("critical",
        conditions: [firing(:mild, "warning", "mild:1", "mildly bad")],
        action_log: log,
        notify: fn _ -> :ok end,
        deliverable: true
      )

    assert result.announced == [], "a warning must not survive a critical floor"
  end

  test "a failed read is an error AND a blindness alert, never health", %{log: log} do
    me = self()

    result =
      Watch.run_poll("warning",
        conditions: [unreachable(:cant_look)],
        action_log: log,
        notify: &collect(&1, me),
        deliverable: true
      )

    assert Enum.all?(result.errors, &(&1 =~ "No route to host"))
    refute result.errors == []

    assert [hit] = result.announced
    assert hit.predicate == "observability_blind"
    assert hit.summary =~ "NOT a report of a healthy fleet"
    assert hit.summary =~ "cant_look"
    assert_received {:announced, [^hit]}
  end

  test "a persistent outage announces once, not once per poll", %{log: log} do
    opts = [
      conditions: [unreachable(:cant_look)],
      action_log: log,
      notify: fn _ -> :ok end,
      deliverable: true
    ]

    assert [_] = Watch.run_poll("warning", opts).announced
    assert Watch.run_poll("warning", opts).announced == []
  end

  test "blindness can be muted; the errors still reach the caller", %{log: log} do
    assert :ok = ActionLog.mute_fleet_predicate("observability_blind", nil, log)

    result =
      Watch.run_poll("warning",
        conditions: [unreachable(:cant_look)],
        action_log: log,
        notify: fn _ -> :ok end,
        deliverable: true
      )

    assert result.announced == []
    refute result.errors == []
  end

  test "blindness ignores the level floor", %{log: log} do
    result =
      Watch.run_poll("emergency",
        conditions: [unreachable(:cant_look)],
        action_log: log,
        notify: fn _ -> :ok end,
        deliverable: true
      )

    assert [%{predicate: "observability_blind"}] = result.announced
  end

  test "a level-filtered hit is consumed, so lowering the floor cannot replay it",
       %{log: log} do
    opts = [
      conditions: [firing(:mild, "warning", "mild:1", "mildly bad")],
      action_log: log,
      notify: fn _ -> :ok end,
      deliverable: true
    ]

    assert Watch.run_poll("critical", opts).announced == []
    assert [%{predicate: "mild"}] = ActionLog.fleet_alerts_seen(log)

    assert Watch.run_poll("warning", opts).announced == [],
           "a deliberately filtered hit must not come back when the floor drops"
  end

  test "nothing is recorded as seen when no transport can receive it", %{log: log} do
    opts = [
      conditions: [firing(:lonely, "critical", "lonely:1", "nobody listening")],
      action_log: log,
      notify: fn _ -> :ok end
    ]

    assert Watch.run_poll("warning", Keyword.put(opts, :deliverable, false)).announced == []
    assert ActionLog.fleet_alerts_seen(log) == [], "an undelivered alert must stay unrecorded"
    assert [_] = Watch.run_poll("warning", Keyword.put(opts, :deliverable, true)).announced
  end

  test "a recovered read re-arms blindness, so a second outage is not silent", %{log: log} do
    opts = [action_log: log, notify: fn _ -> :ok end, deliverable: true]

    blind = [conditions: [unreachable(:cant_look)]]
    healthy = [conditions: [quiet(:cant_look)]]

    assert [_] = Watch.run_poll("warning", blind ++ opts).announced
    assert Watch.run_poll("warning", blind ++ opts).announced == []

    Watch.run_poll("warning", healthy ++ opts)

    assert [_] = Watch.run_poll("warning", blind ++ opts).announced
  end

  test "watch_warnings is opt-in, deduplicated, and names the first watcher" do
    assert :ok = IxMcp.Fleet.watch_warnings("session A")
    assert {:already_watching, "session A"} = IxMcp.Fleet.watch_warnings("session B")
    assert :ok = IxMcp.Fleet.unwatch_warnings()
    assert WarningsWatch.watcher() == nil
  end
end
