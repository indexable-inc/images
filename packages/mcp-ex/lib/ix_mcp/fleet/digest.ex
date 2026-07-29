defmodule IxMcp.Fleet.Digest do
  @moduledoc """
  The journal, as counts, on two schedules: an hourly heartbeat that makes the
  baseline visible, and an immediate anomaly line when a minute is genuinely
  out of band.

  ## Why counts, and why two rates

  Forwarding journal warnings as events is a firehose: the fleet writes about
  22,600 journal rows a minute at the median. Aggregated they are one line.

  But the cadence matters as much as the aggregation, and the first version of
  this module got it wrong. A 60-second unconditional digest emits on 87.1% of
  minutes -- roughly **1,250 lines a day**, one every seventy seconds, at which
  point it is not wallpaper but the thing wallpaper becomes when there is too
  much of it. This repository has two proofs of what happens next: a watchdog
  ignored through 100+ consecutive correct failures, and a fork sync red on
  every run for four days.

  So: **hourly heartbeat** (24 a day, the visible baseline) plus **immediate
  anomaly** (measured at 10.3 a day). Measurement stays per-minute, because
  that is the granularity the data supports; only emission is split.

  ## The measurements this design rests on

  Per-minute counts of warning-and-worse across the fleet, over 7 days:

  | | mean | p50 | p90 | p99 | max |
  |---|---|---|---|---|---|
  | all levels | 22,612 | 12,749 | 39,399 | 160,877 | 663,973 |
  | warning and worse | 173.7 | **33** | 600 | 1,415 | 19,670 |

  Per **hour**: p50 29,136, p90 61,078, p99 251,619, and **zero empty hours in
  169 sampled**. So the heartbeat is 24 lines a day and its suppress-when-empty
  branch will effectively never fire -- stated because "silent when healthy"
  is the rule this module is judged by, and at hourly granularity this fleet is
  never silent.

  ## Why the anomaly test is a quantile and not a ratio

  This is the part that had to be measured rather than reasoned, and reasoning
  got it wrong twice.

  A fixed threshold is hopeless: p50 33 against p99 1,415 is a **43x spread
  inside normal**, so any constant marks a large fraction of healthy minutes.

  A **mean** is what the next person will reach for, and it is worse than
  either: the distribution's tail (max 19,670 against a median of 33) drags the
  mean to 174, five times the median, so half of all normal minutes sit below
  "average" and the ones above it are mostly still normal.

  A **ratio against a rolling median** is what this module shipped first, and
  it does not work either. Simulated over 7 days of real per-host data, "at
  least 20 lines and more than 5x the preceding hour's median" fires **605.7
  times a day**. Tightening barely helps -- 10x with a floor of 200 still fires
  **376 a day** -- because per-host minute counts are spiky, so a rolling
  median sits low and almost any burst clears any multiple of it. The rule is
  structurally wrong, not mis-tuned.

  What works is to **define the threshold as a quantile of the historical
  distribution, so the firing rate is chosen rather than discovered.** A minute
  is anomalous when it exceeds the `q`th percentile of the same clock hour over
  the preceding week; the expected rate is then `(1 - q)` of all minutes.
  Measured exceedances per day:

  | threshold | fires/day |
  |---|---|
  | p99 | 17.1 |
  | **p99.5 (default)** | **10.3** |
  | p99.9 | 3.4 |

  Same clock hour rather than the preceding hour is deliberate: it is the
  comparison that survives a fleet whose load has daily shape, and unlike a
  rolling window it does not let a slow ramp raise its own baseline. The
  per-hour thresholds differ by an order of magnitude (hour 2 is 3,121, hour 4
  is 45,470), which is exactly why one global number cannot work.

  The threshold is cached per clock hour. Recomputing it costs a 7-day scan,
  which is fine hourly and not fine every 60 seconds.
  """

  alias IxMcp.Fleet.ClickHouse

  # The fleet's journal uses "warn", not "warning" -- verified against
  # logs.journald_logs, whose only levels are info, notice, warn, error, crit,
  # debug and alert. Carrying both spellings meant render/1 could print
  # "5 warnings, 3 warnings", and it is what forced the one-way mapping in
  # Watch.drop_muted_levels/2. The mute id stays "digest:warning" because that
  # is what an operator types; the mapping happens there, once.
  @notable ~w(warn error crit alert emerg)

  # Quantile defining "out of band". Chosen for its measured consequence --
  # 10.3 lines a day -- rather than for looking principled.
  @anomaly_quantile 0.995

  # Below this, a percentile is meaningless: on a quiet hour the p99.5 can be a
  # handful of lines, and exceeding it is not news.
  @anomaly_floor 50

  @type counts :: %{String.t() => non_neg_integer()}

  @type t :: %{
          from: String.t(),
          to: String.t(),
          period_s: pos_integer(),
          counts: counts(),
          total: non_neg_integer()
        }

  @type anomaly :: %{
          minute: String.t(),
          count: non_neg_integer(),
          threshold: float(),
          hosts: [{String.t(), non_neg_integer()}]
        }

  @doc "The quantile the anomaly threshold is taken at."
  @spec anomaly_quantile() :: float()
  def anomaly_quantile, do: @anomaly_quantile

  # -- heartbeat ---------------------------------------------------------------

  @doc """
  Summarise the last `period_s` seconds as counts by severity.

  `{:ok, nil}` for an empty window (silence must still mean healthy, even
  though at hourly granularity this fleet has never produced one),
  `{:error, reason}` when the window could not be read -- which is not the
  same as an empty one.
  """
  @spec build(pos_integer(), (String.t() -> {:ok, [map()]} | {:error, String.t()})) ::
          {:ok, t() | nil} | {:error, String.t()}
  def build(period_s, query_fun \\ &ClickHouse.query/1) do
    with {:ok, rows} <- query_fun.(window_sql(period_s)) do
      summarise(rows, period_s)
    end
  end

  defp summarise(rows, period_s) do
    counts = counts_of(rows)
    total = counts |> Map.values() |> Enum.sum()

    if total == 0 do
      {:ok, nil}
    else
      {:ok,
       %{
         from: to_string(period_start(rows)),
         to: to_string(period_end(rows)),
         period_s: period_s,
         counts: counts,
         total: total
       }}
    end
  end

  @doc "One line: counts by severity, over the heartbeat window."
  @spec render(t()) :: String.t()
  def render(digest) do
    parts =
      @notable
      |> Enum.map(fn level -> {level, Map.get(digest.counts, level, 0)} end)
      |> Enum.reject(fn {_level, n} -> n == 0 end)
      |> Enum.map_join(", ", fn {level, n} -> "#{n} #{plural(level, n)}" end)

    window = humanise(digest.period_s)

    case parts do
      "" -> "#{digest.total} notable in the last #{window}"
      text -> "#{text} in the last #{window}"
    end
  end

  defp humanise(s) when s >= 3_600, do: "#{div(s, 3_600)}h"
  defp humanise(s) when s >= 60, do: "#{div(s, 60)}m"
  defp humanise(s), do: "#{s}s"

  defp plural("warn", n), do: plural("warning", n)
  defp plural(level, 1), do: level
  defp plural(level, _n), do: level <> "s"

  # -- anomaly -----------------------------------------------------------------

  @doc """
  Threshold for the given clock hour: the `q`th percentile of per-minute
  warning-and-worse counts in that hour over the preceding week.

  Cached by the caller per clock hour -- this is a 7-day scan.
  """
  @spec threshold(non_neg_integer(), (String.t() -> {:ok, [map()]} | {:error, String.t()})) ::
          {:ok, float()} | {:error, String.t()}
  def threshold(hour, query_fun \\ &ClickHouse.query/1) do
    sql = """
    SELECT quantile(#{@anomaly_quantile})(n) AS threshold
    FROM (
      SELECT toStartOfMinute(timestamp) AS m, count() AS n
      FROM logs.journald_logs
      WHERE timestamp > now() - INTERVAL 7 DAY
        AND toHour(timestamp) = #{hour}
        AND level IN (#{level_list()})
      GROUP BY m
    )
    """

    # A threshold of 0.0 disables detection for the whole hour, because
    # `judge/2` requires `threshold > 0`. So a missing or unparseable answer
    # must be an error, never a number: decaying to 0.0 would read as "nothing
    # was out of band this hour" when what happened is that we never looked.
    case query_fun.(sql) do
      {:ok, [%{"threshold" => t} | _]} when is_number(t) -> {:ok, t * 1.0}
      {:ok, [%{"threshold" => t} | _]} when is_binary(t) -> parse_threshold(t, hour)
      {:ok, [%{"threshold" => nil} | _]} -> {:error, no_history(hour)}
      {:ok, []} -> {:error, no_history(hour)}
      {:ok, other} -> {:error, "unexpected threshold row for hour #{hour}: #{inspect(other)}"}
      {:error, reason} -> {:error, reason}
    end
  end

  defp no_history(hour),
    do: "no p#{@anomaly_quantile * 100} for hour #{hour}: no journal history in the last 7 days"

  defp parse_threshold(text, hour) do
    case Float.parse(text) do
      {value, _rest} -> {:ok, value}
      :error -> {:error, "unparseable threshold #{inspect(text)} for hour #{hour}"}
    end
  end

  @doc """
  The clock hour the next `check_anomaly/2` will measure.

  Not `DateTime.utc_now().hour`: the window is the last COMPLETE minute, so at
  HH:00:ss it belongs to HH-1. Scoring it against HH's threshold is wrong 24
  times a day, deterministically, and wrong by a lot -- hour 1's p99.5 is
  33,448 against hour 2's 3,121, so the boundary minute is either a guaranteed
  false alarm or a guaranteed miss depending on which way the step goes.
  """
  @spec measured_hour(DateTime.t()) :: non_neg_integer()
  def measured_hour(now \\ DateTime.utc_now()),
    do: now |> DateTime.add(-60, :second) |> Map.fetch!(:hour)

  @doc """
  Check the last complete minute against `threshold`. `{:ok, nil}` when it is
  in band, which is the overwhelmingly common case.

  Per-host counts ride along on a hit: fleet-wide is where the signal lives
  (per-host means are 0.1 to 9.6 a minute, so a per-host line would be
  near-silent), but once something has fired, naming the culprit is the whole
  value of the line.
  """
  @spec check_anomaly(float(), (String.t() -> {:ok, [map()]} | {:error, String.t()})) ::
          {:ok, anomaly() | nil} | {:error, String.t()}
  def check_anomaly(threshold, query_fun \\ &ClickHouse.query/1) do
    sql = """
    SELECT node_id, count() AS n, min(toStartOfMinute(timestamp)) AS minute
    FROM logs.journald_logs
    WHERE timestamp >= toStartOfMinute(now() - INTERVAL 1 MINUTE)
      AND timestamp < toStartOfMinute(now())
      AND level IN (#{level_list()})
    GROUP BY node_id
    """

    with {:ok, rows} <- query_fun.(sql) do
      {:ok, judge(rows, threshold)}
    end
  end

  defp judge([], _threshold), do: nil

  defp judge(rows, threshold) do
    total = rows |> Enum.map(&as_int(&1["n"])) |> Enum.sum()

    if total >= @anomaly_floor and threshold > 0 and total > threshold do
      hosts =
        rows
        |> Enum.map(&{&1["node_id"], as_int(&1["n"])})
        |> Enum.sort_by(&elem(&1, 1), :desc)
        |> Enum.take(3)

      %{
        minute: rows |> Enum.map(& &1["minute"]) |> Enum.min() |> to_string(),
        count: total,
        threshold: threshold,
        hosts: hosts
      }
    end
  end

  @doc "One loud line for an out-of-band minute, naming the culprit hosts."
  @spec render_anomaly(anomaly()) :: String.t()
  def render_anomaly(anomaly) do
    who =
      Enum.map_join(anomaly.hosts, ", ", fn {node, n} -> "#{node} #{n}" end)

    ratio = if anomaly.threshold > 0, do: anomaly.count / anomaly.threshold, else: 0.0

    "#{anomaly.count} notable in one minute, over the #{quantile_label()} " <>
      "threshold of #{round(anomaly.threshold)} for this hour (#{Float.round(ratio, 1)}x) -- #{who}"
  end

  defp quantile_label, do: "p#{Float.round(@anomaly_quantile * 100, 1)}"

  # -- shared ------------------------------------------------------------------

  defp window_sql(period_s) do
    """
    SELECT level, count() AS n, min(timestamp) AS from_ts, max(timestamp) AS to_ts
    FROM logs.journald_logs
    WHERE timestamp > now() - INTERVAL #{period_s} SECOND
      AND level IN (#{level_list()})
    GROUP BY level
    """
  end

  defp level_list, do: Enum.map_join(@notable, ", ", &"'#{&1}'")

  defp counts_of(rows), do: Map.new(rows, fn row -> {row["level"], as_int(row["n"])} end)

  defp period_start(rows), do: rows |> Enum.map(& &1["from_ts"]) |> Enum.min(fn -> "" end)
  defp period_end(rows), do: rows |> Enum.map(& &1["to_ts"]) |> Enum.max(fn -> "" end)

  # ClickHouse JSONEachRow returns UInt64 as a string and smaller ints as
  # numbers, so neither shape can be assumed.
  defp as_int(v) when is_integer(v), do: v
  defp as_int(v) when is_binary(v), do: String.to_integer(v)
  defp as_int(v) when is_float(v), do: trunc(v)
  defp as_int(_), do: 0

  @doc """
  The detail behind a heartbeat or an anomaly: what was actually counted in
  `from`..`to`.

  The line is a pointer, not the content. Without this, getting curious means
  writing ClickHouse by hand at precisely the moment attention is available.
  """
  @spec detail(String.t(), String.t(), (String.t() -> {:ok, [map()]} | {:error, String.t()})) ::
          {:ok, [map()]} | {:error, String.t()}
  def detail(from, to, query_fun \\ &ClickHouse.query/1) do
    query_fun.("""
    SELECT node_id, level, systemd_unit, count() AS n, any(message) AS sample
    FROM logs.journald_logs
    WHERE timestamp >= '#{escape(from)}' AND timestamp <= '#{escape(to)}'
      AND level IN (#{level_list()})
    GROUP BY node_id, level, systemd_unit
    ORDER BY n DESC
    LIMIT 40
    """)
  end

  # These come from our own rows, not user input, but they are interpolated
  # into SQL: a quote arriving here would be a bug elsewhere and must not
  # become an injection.
  defp escape(text), do: String.replace(text, ~r/[^0-9\-: .]/, "")
end
