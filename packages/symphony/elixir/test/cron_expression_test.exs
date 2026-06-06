defmodule SymphonyElixir.CronExpressionTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.CronExpression

  describe "parse/1" do
    test "accepts the nicknames" do
      for nick <- ~w(@yearly @annually @monthly @weekly @daily @midnight @hourly) do
        assert {:ok, parsed} = CronExpression.parse(nick)
        assert parsed.source == nick
      end
    end

    test "trims whitespace before resolving nicknames" do
      assert {:ok, parsed} = CronExpression.parse("  @monthly  ")
      assert parsed.source == "@monthly"
    end

    test "accepts standard 5-field cron strings" do
      assert {:ok, _} = CronExpression.parse("0 0 1 * *")
      assert {:ok, _} = CronExpression.parse("*/15 * * * *")
      assert {:ok, _} = CronExpression.parse("0 9-17 * * 1-5")
      assert {:ok, _} = CronExpression.parse("0,15,30,45 * * * *")
    end

    test "rejects malformed expressions" do
      assert {:error, _} = CronExpression.parse("not a cron")
      assert {:error, _} = CronExpression.parse("0 0 1 *")
      assert {:error, _} = CronExpression.parse("60 0 1 * *")
      assert {:error, _} = CronExpression.parse("0 24 1 * *")
      assert {:error, _} = CronExpression.parse("0 0 32 * *")
      assert {:error, _} = CronExpression.parse("0 0 1 13 *")
      assert {:error, _} = CronExpression.parse("0 0 1 * 7")
    end

    test "rejects inverted ranges" do
      assert {:error, _} = CronExpression.parse("10-5 0 1 * *")
    end

    test "rejects non-positive step" do
      assert {:error, _} = CronExpression.parse("*/0 0 1 * *")
    end
  end

  describe "next_fire_after/2 with @hourly" do
    test "advances to the next hour boundary" do
      {:ok, parsed} = CronExpression.parse("@hourly")
      from = ~U[2026-05-17 14:23:00Z]
      assert {:ok, ~U[2026-05-17 15:00:00Z]} = CronExpression.next_fire_after(parsed, from)
    end

    test "never returns the from moment itself" do
      {:ok, parsed} = CronExpression.parse("@hourly")
      from = ~U[2026-05-17 14:00:00Z]
      assert {:ok, ~U[2026-05-17 15:00:00Z]} = CronExpression.next_fire_after(parsed, from)
    end
  end

  describe "next_fire_after/2 with @daily" do
    test "advances to midnight UTC the next day" do
      {:ok, parsed} = CronExpression.parse("@daily")
      from = ~U[2026-05-17 14:00:00Z]
      assert {:ok, ~U[2026-05-18 00:00:00Z]} = CronExpression.next_fire_after(parsed, from)
    end
  end

  describe "next_fire_after/2 with @monthly" do
    test "advances to the 1st of the next month at 00:00 UTC" do
      {:ok, parsed} = CronExpression.parse("@monthly")
      from = ~U[2026-05-17 14:00:00Z]
      assert {:ok, ~U[2026-06-01 00:00:00Z]} = CronExpression.next_fire_after(parsed, from)
    end

    test "rolls into the next year correctly" do
      {:ok, parsed} = CronExpression.parse("@monthly")
      from = ~U[2026-12-15 09:30:00Z]
      assert {:ok, ~U[2027-01-01 00:00:00Z]} = CronExpression.next_fire_after(parsed, from)
    end
  end

  describe "next_fire_after/2 with explicit 5-field" do
    test "*/15 * * * * fires on the next quarter-hour" do
      {:ok, parsed} = CronExpression.parse("*/15 * * * *")
      from = ~U[2026-05-17 14:07:00Z]
      assert {:ok, ~U[2026-05-17 14:15:00Z]} = CronExpression.next_fire_after(parsed, from)
    end

    test "weekday business hours respects day-of-week" do
      # 9am on weekdays (Mon-Fri). 2026-05-17 is a Sunday.
      {:ok, parsed} = CronExpression.parse("0 9 * * 1-5")
      from = ~U[2026-05-17 12:00:00Z]
      assert {:ok, ~U[2026-05-18 09:00:00Z]} = CronExpression.next_fire_after(parsed, from)
    end

    test "POSIX OR semantics for DOM and DOW when both restricted" do
      # 'every 1st of the month OR every Friday'
      {:ok, parsed} = CronExpression.parse("0 0 1 * 5")
      # Thursday May 14 2026 -> first match is Friday May 15
      from = ~U[2026-05-14 12:00:00Z]
      assert {:ok, ~U[2026-05-15 00:00:00Z]} = CronExpression.next_fire_after(parsed, from)
    end
  end

  describe "matches?/2" do
    test "@hourly matches every wall-clock hour" do
      {:ok, parsed} = CronExpression.parse("@hourly")
      assert CronExpression.matches?(parsed, ~U[2026-05-17 03:00:00Z])
      refute CronExpression.matches?(parsed, ~U[2026-05-17 03:01:00Z])
    end
  end
end
