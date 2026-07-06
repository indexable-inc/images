defmodule SymphonyElixir.Triggers.CronTest do
  # The loud-skip test boots the shared named singletons (WorkflowCatalog,
  # CronState, Triggers.Cron), so this file cannot run concurrently with
  # other tests that start them.
  use ExUnit.Case, async: false

  import ExUnit.CaptureLog

  alias SymphonyElixir.{CronExpression, WorkflowCatalog}
  alias SymphonyElixir.Triggers.Cron

  # US DST in 2026: spring forward Sun Mar 8 (02:00 PST -> 03:00 PDT),
  # fall back Sun Nov 1 (02:00 PDT -> 01:00 PST). PST=UTC-8, PDT=UTC-7.
  @la "America/Los_Angeles"

  defp parsed!(schedule) do
    {:ok, parsed} = CronExpression.parse(schedule)
    parsed
  end

  describe "next_due/4 plain zone offset" do
    test "9am LA is 17:00 UTC in winter (PST)" do
      last = ~U[2026-01-15 17:30:00Z]
      now = ~U[2026-01-16 17:00:30Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("0 9 * * *"), last, now, @la)
      assert DateTime.compare(scheduled_for, ~U[2026-01-16 17:00:00Z]) == :eq
      assert DateTime.to_iso8601(scheduled_for) == "2026-01-16T09:00:00-08:00"
    end

    test "9am LA is 16:00 UTC in summer (PDT)" do
      last = ~U[2026-07-15 16:30:00Z]
      now = ~U[2026-07-16 16:00:30Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("0 9 * * *"), last, now, @la)
      assert DateTime.compare(scheduled_for, ~U[2026-07-16 16:00:00Z]) == :eq
      assert DateTime.to_iso8601(scheduled_for) == "2026-07-16T09:00:00-07:00"
    end

    test "not due before the local moment arrives" do
      last = ~U[2026-07-15 16:30:00Z]
      now = ~U[2026-07-16 15:59:00Z]

      assert Cron.next_due(parsed!("0 9 * * *"), last, now, @la) == :not_due
    end

    test "the UTC default zone behaves like plain UTC cron" do
      last = ~U[2026-05-17 08:31:00Z]
      now = ~U[2026-05-17 09:00:10Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("0 9 * * *"), last, now, "UTC")
      assert DateTime.compare(scheduled_for, ~U[2026-05-17 09:00:00Z]) == :eq
    end
  end

  describe "next_due/4 spring-forward gap" do
    test "a 02:30 that does not exist fires at the first valid instant after the gap" do
      # Fired Mar 7 at 02:30 PST; the watermark sits just after it.
      last = ~U[2026-03-07 10:31:00Z]
      # 03:00 PDT (the gap's end) is 10:00 UTC.
      now = ~U[2026-03-08 10:00:30Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("30 2 * * *"), last, now, @la)
      assert DateTime.compare(scheduled_for, ~U[2026-03-08 10:00:00Z]) == :eq
      assert DateTime.to_iso8601(scheduled_for) == "2026-03-08T03:00:00-07:00"
    end

    test "the gap fire is not due before the transition moment" do
      last = ~U[2026-03-07 10:31:00Z]
      now = ~U[2026-03-08 09:59:00Z]

      assert Cron.next_due(parsed!("30 2 * * *"), last, now, @la) == :not_due
    end
  end

  describe "next_due/4 fall-back repeat" do
    test "a repeated 01:30 fires at the first (earlier) occurrence" do
      last = ~U[2026-10-31 08:31:00Z]
      # First 01:30 is PDT = 08:30 UTC; the repeat (PST) is 09:30 UTC.
      now = ~U[2026-11-01 08:31:00Z]

      assert {:fire, scheduled_for} = Cron.next_due(parsed!("30 1 * * *"), last, now, @la)
      assert DateTime.compare(scheduled_for, ~U[2026-11-01 08:30:00Z]) == :eq
      assert DateTime.to_iso8601(scheduled_for) == "2026-11-01T01:30:00-07:00"
    end

    test "the watermark from the first occurrence swallows the repeat: one fire, not two" do
      # The fire above records last_fired_at = now (01:31 PDT). When the
      # wall clock reads 01:30 again an hour later (PST), the next
      # wall-clock match after the watermark is tomorrow's 01:30.
      watermark = ~U[2026-11-01 08:31:00Z]
      second_occurrence = ~U[2026-11-01 09:30:30Z]

      assert Cron.next_due(parsed!("30 1 * * *"), watermark, second_occurrence, @la) == :not_due
    end
  end

  describe "next_due/4 unknown zone" do
    test "an unknown zone is an error, never a silent UTC fallback" do
      last = ~U[2026-01-15 17:30:00Z]
      now = ~U[2026-01-16 17:00:30Z]

      assert Cron.next_due(parsed!("0 9 * * *"), last, now, "Not/AZone") ==
               {:error, :time_zone_not_found}
    end
  end

  describe "tick with an unknown zone" do
    @tag :tmp_dir
    test "logs a warning and keeps ticking instead of crashing", %{tmp_dir: dir} do
      File.write!(
        Path.join(dir, "badtz.sym"),
        ~s|workflow "badtz" on cron "* * * * *" tz "Not/AZone" { a <- exec "./x.sh" }|
      )

      start_supervised!({WorkflowCatalog, workflows_dir: dir, poll_ms: 60_000})
      start_supervised!(SymphonyElixir.CronState)
      cron = start_supervised!({Cron, []})

      # The catalog's boot scan is an async message; scan synchronously so
      # the first poll below deterministically sees the workflow.
      :ok = WorkflowCatalog.scan(dir)

      # First poll seeds the watermark; the second evaluates against it and
      # hits the zone lookup.
      log =
        capture_log(fn ->
          :ok = Cron.poll_now()
          :ok = Cron.poll_now()
        end)

      assert log =~ "Cron timezone unknown for workflow=badtz"
      assert log =~ "Not/AZone"
      assert Process.alive?(cron)
    end
  end
end
