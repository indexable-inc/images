defmodule SymphonyElixir.CronExpression do
  @moduledoc """
  Tiny cron parser and next-fire calculator for the `cron` trigger kind.

  Accepts:

  - Standard 5-field cron strings: `minute hour day-of-month month day-of-week`.
    Each field supports `*`, a single integer, a comma-separated list
    (`1,3,5`), a range (`1-5`), and the step form (`*/15`, `0-30/5`).
  - Nicknames: `@yearly`, `@annually`, `@monthly`, `@weekly`, `@daily`,
    `@midnight`, `@hourly`.

  Time is treated as UTC. Day-of-week is 0..6 with 0=Sunday, matching
  POSIX cron.

  When both `day-of-month` and `day-of-week` are restricted (neither is
  `*`), the match is the OR of the two, again matching POSIX cron. When
  exactly one is restricted, only that one constrains the match. When
  both are `*`, any day matches.

  ## Usage

      {:ok, parsed} = CronExpression.parse("@monthly")
      CronExpression.next_fire_after(parsed, DateTime.utc_now())
      # => %DateTime{...}  (the first minute of the next month, UTC)

  `next_fire_after/2` returns the first matching minute strictly after
  the given moment. It steps minute-by-minute and bails out after two
  years, treating an exhausted search as `{:error, :no_match_within_window}`.
  """

  @two_years_in_minutes 525_600 * 2

  @type field :: MapSet.t(non_neg_integer())

  @type t :: %{
          required(:source) => String.t(),
          required(:minute) => field(),
          required(:hour) => field(),
          required(:dom) => field(),
          required(:month) => field(),
          required(:dow) => field(),
          required(:dom_restricted?) => boolean(),
          required(:dow_restricted?) => boolean()
        }

  @nicknames %{
    "@yearly" => "0 0 1 1 *",
    "@annually" => "0 0 1 1 *",
    "@monthly" => "0 0 1 * *",
    "@weekly" => "0 0 * * 0",
    "@daily" => "0 0 * * *",
    "@midnight" => "0 0 * * *",
    "@hourly" => "0 * * * *"
  }

  @spec parse(String.t()) :: {:ok, t()} | {:error, term()}
  def parse(expr) when is_binary(expr) do
    trimmed = String.trim(expr)

    case Map.fetch(@nicknames, trimmed) do
      {:ok, expanded} -> do_parse(expanded, trimmed)
      :error -> do_parse(trimmed, trimmed)
    end
  end

  # astlog-ignore: public-def-needs-spec
  def parse(_), do: {:error, :invalid_cron_expression}

  defp do_parse(expanded, source) do
    case String.split(expanded, ~r/\s+/, trim: true) do
      [minute, hour, dom, month, dow] ->
        with {:ok, m} <- parse_field(minute, 0..59),
             {:ok, h} <- parse_field(hour, 0..23),
             {:ok, d} <- parse_field(dom, 1..31),
             {:ok, mo} <- parse_field(month, 1..12),
             {:ok, w} <- parse_field(dow, 0..6) do
          {:ok,
           %{
             source: source,
             minute: m,
             hour: h,
             dom: d,
             month: mo,
             dow: w,
             dom_restricted?: dom != "*",
             dow_restricted?: dow != "*"
           }}
        end

      _ ->
        {:error, {:invalid_cron_expression, source}}
    end
  end

  defp parse_field(field, range) do
    field
    |> String.split(",", trim: true)
    |> Enum.reduce_while({:ok, MapSet.new()}, fn part, {:ok, acc} ->
      case parse_part(part, range) do
        {:ok, values} -> {:cont, {:ok, MapSet.union(acc, values)}}
        {:error, _} = err -> {:halt, err}
      end
    end)
  end

  # Step form: `<base>/<step>`. Base may itself be `*`, a single integer,
  # or a range.
  defp parse_part(part, range) do
    case String.split(part, "/", parts: 2) do
      [base, step_str] ->
        with {:ok, step} <- parse_step(step_str),
             {:ok, base_values} <- parse_base(base, range) do
          values = base_values |> Enum.sort() |> apply_step(step) |> MapSet.new()
          {:ok, values}
        end

      [base] ->
        parse_base(base, range)
    end
  end

  defp parse_step(s) do
    case Integer.parse(s) do
      {n, ""} when n > 0 -> {:ok, n}
      _ -> {:error, {:invalid_step, s}}
    end
  end

  defp parse_base("*", range), do: {:ok, MapSet.new(range)}

  defp parse_base(base, range) do
    case String.split(base, "-", parts: 2) do
      [a, b] ->
        with {:ok, lo} <- parse_int_in(a, range),
             {:ok, hi} <- parse_int_in(b, range) do
          if lo <= hi do
            {:ok, MapSet.new(lo..hi)}
          else
            {:error, {:invalid_range, base}}
          end
        end

      [single] ->
        case parse_int_in(single, range) do
          {:ok, n} -> {:ok, MapSet.new([n])}
          {:error, _} = err -> err
        end
    end
  end

  defp parse_int_in(s, range) do
    case Integer.parse(s) do
      {n, ""} ->
        if n in range, do: {:ok, n}, else: {:error, {:out_of_range, s, Enum.min(range), Enum.max(range)}}

      _ ->
        {:error, {:invalid_integer, s}}
    end
  end

  defp apply_step(sorted_values, step) when is_list(sorted_values) and step > 0 do
    case sorted_values do
      [] ->
        []

      [first | _] ->
        sorted_values
        |> Enum.filter(fn v -> rem(v - first, step) == 0 end)
    end
  end

  @doc """
  Returns the first DateTime strictly after `from` whose minute matches
  the parsed expression. Resolution is one minute; sub-minute components
  of `from` are floored before stepping.
  """
  @spec next_fire_after(t(), DateTime.t()) :: {:ok, DateTime.t()} | {:error, term()}
  def next_fire_after(%{} = parsed, %DateTime{} = from) do
    floored =
      from
      |> DateTime.shift_zone!("Etc/UTC")
      |> truncate_to_minute()

    # Advance one minute past `from` so we never return `from` itself.
    start = DateTime.add(floored, 60, :second)
    step(start, parsed, 0)
  end

  defp step(_dt, _parsed, n) when n > @two_years_in_minutes do
    {:error, :no_match_within_window}
  end

  defp step(%DateTime{} = dt, parsed, n) do
    if matches?(parsed, dt) do
      {:ok, dt}
    else
      step(DateTime.add(dt, 60, :second), parsed, n + 1)
    end
  end

  @doc """
  Whether the given DateTime's minute matches the parsed expression.
  """
  @spec matches?(t(), DateTime.t()) :: boolean()
  def matches?(parsed, %DateTime{} = dt) do
    dt = DateTime.shift_zone!(dt, "Etc/UTC")
    date = DateTime.to_date(dt)

    minute_ok = MapSet.member?(parsed.minute, dt.minute)
    hour_ok = MapSet.member?(parsed.hour, dt.hour)
    month_ok = MapSet.member?(parsed.month, date.month)

    day_ok = day_matches?(parsed, date)

    minute_ok and hour_ok and month_ok and day_ok
  end

  # POSIX cron day matching:
  # - If both DOM and DOW are *, every day matches (and both sets are full).
  # - If only one of DOM/DOW is restricted, only that one constrains.
  # - If both are restricted, match if EITHER matches (OR, not AND).
  defp day_matches?(parsed, %Date{} = date) do
    dom_match = MapSet.member?(parsed.dom, date.day)
    dow_match = MapSet.member?(parsed.dow, day_of_week_sunday0(date))

    cond do
      parsed.dom_restricted? and parsed.dow_restricted? -> dom_match or dow_match
      parsed.dom_restricted? -> dom_match
      parsed.dow_restricted? -> dow_match
      true -> true
    end
  end

  # Date.day_of_week/1 returns 1..7 with Monday=1; cron uses 0..6 with
  # Sunday=0. Convert by `rem(date.day_of_week(), 7)`.
  defp day_of_week_sunday0(%Date{} = d), do: rem(Date.day_of_week(d), 7)

  defp truncate_to_minute(%DateTime{} = dt) do
    %{dt | second: 0, microsecond: {0, 0}}
  end
end
