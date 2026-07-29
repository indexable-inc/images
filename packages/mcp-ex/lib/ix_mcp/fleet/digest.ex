defmodule IxMcp.Fleet.Digest do
  @moduledoc """
  The periodic rolled-up summary of the fleet's journal: one line per period,
  counts by severity, anomalies marked inline against a rolling baseline.

  ## Why counts rather than events

  Forwarding journal warnings as individual notifications is a firehose: the
  fleet writes roughly 22,600 journal rows a minute at the median and 664,000
  at the observed peak. Aggregated, the same information is one line. So the
  class that had to be rejected on volume grounds comes back as counts.

  ## What a normal minute contains

  Measured over 7 days of `logs.journald_logs`, per-minute counts:

  | | mean | p50 | p90 | p99 | max |
  |---|---|---|---|---|---|
  | all levels | 22,612 | 12,749 | 39,399 | 160,877 | 663,973 |
  | warning and worse | 173.7 | **33** | 600 | 1,415 | 19,670 |
  | warning | 435 | 92 | 856 | 3,648 | 97,856 |
  | error | 200 | 43 | 658 | 1,489 | 19,670 |
  | crit | 14.6 | 2 | 12 | 383 | 756 |

  Three things follow, and each one shaped the code below.

  **A minute is the right period.** 33 notable lines is a digestible one-liner.

  **A fixed anomaly threshold would be useless.** p50 33 to p99 1,415 is a 43x
  spread *inside normal*. Marking anomalies against a mean would flag half of
  all healthy minutes, so the baseline is a rolling quantile and the test is a
  ratio against it, with an absolute floor so a jump from 1 to 6 is not news.

  **87.1% of minutes are non-empty** (1,296 of 10,081 sampled minutes were
  zero). At a 60s period that is roughly 1,250 digests a day. That is the
  intended wallpaper -- a steady number is information once and furniture
  afterwards, and the furniture is what makes an abnormal number visible -- but
  it is a real cost, which is why `Fleet.digest_period/1` exists and why the
  measured figure is in the tool description rather than left for the next
  person to rediscover.

  ## Baseline choice, and its limit

  The baseline is the per-minute median of the preceding hour, not the same
  clock-hour on previous days. It is one cheap query, it adapts to a fleet
  whose load changes shape during the day, and it answers the question an
  operator in a session actually has: is this minute unlike the last hour.

  The limit is explicit: **a slow ramp hides from it.** A fleet degrading over
  six hours moves its own baseline along with it and never trips the ratio.
  Catching that needs a same-hour-yesterday comparison, which is a heavier
  query over a 244M-row table; it is deliberately not done here, and anyone who
  needs it should know that is the gap rather than assume it is covered.
  """

  alias IxMcp.Fleet.ClickHouse

  @notable ~w(warning warn error crit alert emerg)

  # Anomaly test. Both conditions, because either alone is wrong: a ratio
  # without a floor fires when 1 becomes 6, and a floor without a ratio fires
  # on every busy-but-normal minute.
  @anomaly_ratio 5.0
  @anomaly_floor 20

  @type counts :: %{String.t() => non_neg_integer()}

  @type t :: %{
          from: String.t(),
          to: String.t(),
          counts: counts(),
          total: non_neg_integer(),
          anomalies: [String.t()],
          baseline_per_min: float()
        }

  @doc """
  Summarise the last `period_s` seconds. `{:ok, nil}` means the window was
  empty and nothing should be said; `{:ok, digest}` is a window worth a line;
  `{:error, reason}` means the window could not be read, which is not the same
  as an empty one.
  """
  @spec build(pos_integer(), (String.t() -> {:ok, [map()]} | {:error, String.t()})) ::
          {:ok, t() | nil} | {:error, String.t()}
  def build(period_s, query_fun \\ &ClickHouse.query/1) do
    with {:ok, rows} <- query_fun.(window_sql(period_s)) do
      summarise(rows, period_s, query_fun)
    end
  end

  defp summarise(rows, period_s, query_fun) do
    counts = counts_of(rows)
    total = counts |> Map.values() |> Enum.sum()

    if total == 0 do
      # Silence still has to mean healthy. An empty window says nothing at all
      # rather than "0 warnings, 0 errors", which would be a line every minute
      # forever saying that nothing happened.
      {:ok, nil}
    else
      with_baseline(rows, counts, total, period_s, query_fun)
    end
  end

  defp with_baseline(rows, counts, total, period_s, query_fun) do
    with {:ok, baseline} <- baseline(period_s, query_fun),
         {:ok, hosts} <- host_anomalies(period_s, baseline, query_fun) do
      {:ok,
       %{
         from: to_string(period_start(rows)),
         to: to_string(period_end(rows)),
         counts: counts,
         total: total,
         anomalies: hosts,
         baseline_per_min: baseline
       }}
    end
  end

  @doc """
  One line for an operator. Counts, then anomalies, then how to stop it.
  """
  @spec render(t()) :: String.t()
  def render(digest) do
    parts =
      @notable
      |> Enum.map(fn level -> {level, Map.get(digest.counts, level, 0)} end)
      |> Enum.reject(fn {_level, n} -> n == 0 end)
      |> Enum.map_join(", ", fn {level, n} -> "#{n} #{plural(level, n)}" end)

    base = if parts == "", do: "#{digest.total} notable", else: parts

    case digest.anomalies do
      [] -> base
      marks -> base <> " -- " <> Enum.join(marks, "; ")
    end
  end

  defp plural("warn", n), do: plural("warning", n)
  defp plural(level, 1), do: level
  defp plural(level, _n), do: level <> "s"

  # -- queries -----------------------------------------------------------------

  defp window_sql(period_s) do
    """
    SELECT level, count() AS n, min(timestamp) AS from_ts, max(timestamp) AS to_ts
    FROM logs.journald_logs
    WHERE timestamp > now() - INTERVAL #{period_s} SECOND
      AND level IN (#{level_list()})
    GROUP BY level
    """
  end

  # Per-minute median over the preceding hour, excluding the window being
  # judged so a spike cannot raise its own baseline.
  defp baseline(period_s, query_fun) do
    sql = """
    SELECT quantile(0.5)(n) AS p50
    FROM (
      SELECT toStartOfMinute(timestamp) AS m, count() AS n
      FROM logs.journald_logs
      WHERE timestamp > now() - INTERVAL 1 HOUR
        AND timestamp <= now() - INTERVAL #{period_s} SECOND
        AND level IN (#{level_list()})
      GROUP BY m
    )
    """

    case query_fun.(sql) do
      {:ok, [%{"p50" => p50} | _]} -> {:ok, as_float(p50)}
      {:ok, _empty} -> {:ok, 0.0}
      {:error, reason} -> {:error, reason}
    end
  end

  # A host is called out only when it is disproportionate, not merely present:
  # naming every host every minute is how the line stops being read.
  defp host_anomalies(period_s, baseline, query_fun) do
    sql = """
    SELECT node_id, count() AS n
    FROM logs.journald_logs
    WHERE timestamp > now() - INTERVAL #{period_s} SECOND
      AND level IN (#{level_list()})
    GROUP BY node_id
    ORDER BY n DESC
    LIMIT 5
    """

    case query_fun.(sql) do
      {:ok, rows} -> {:ok, Enum.flat_map(rows, &mark(&1, baseline, period_s))}
      {:error, reason} -> {:error, reason}
    end
  end

  defp mark(row, baseline, period_s) do
    n = as_int(row["n"])
    # The fleet-wide baseline is per minute; scale it to this window and to a
    # single host's share, or every host looks quiet next to the whole fleet.
    per_window = baseline * period_s / 60.0
    ratio = if per_window > 0, do: n / per_window, else: 0.0

    if n >= @anomaly_floor and ratio >= @anomaly_ratio do
      ["#{row["node_id"]} #{Float.round(ratio, 1)}x baseline (#{n})"]
    else
      []
    end
  end

  defp level_list, do: Enum.map_join(@notable, ", ", &"'#{&1}'")

  defp counts_of(rows) do
    Map.new(rows, fn row -> {row["level"], as_int(row["n"])} end)
  end

  defp period_start(rows), do: rows |> Enum.map(& &1["from_ts"]) |> Enum.min(fn -> "" end)
  defp period_end(rows), do: rows |> Enum.map(& &1["to_ts"]) |> Enum.max(fn -> "" end)

  # ClickHouse JSONEachRow gives counts as strings for UInt64 and numbers for
  # smaller types, so neither shape can be assumed.
  defp as_int(v) when is_integer(v), do: v
  defp as_int(v) when is_binary(v), do: String.to_integer(v)
  defp as_int(v) when is_float(v), do: trunc(v)
  defp as_int(_), do: 0

  defp as_float(v) when is_float(v), do: v
  defp as_float(v) when is_integer(v), do: v * 1.0
  defp as_float(v) when is_binary(v), do: String.to_float(v)
  defp as_float(_), do: 0.0

  @doc """
  The detail behind a digest: what was actually counted in `from`..`to`.

  The digest is a pointer, not the content. Without this an operator who gets
  curious has to go and write ClickHouse by hand at exactly the moment their
  attention is available, which is the moment you lose them.
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

  # These values come from our own digest rows rather than user input, but they
  # are interpolated into SQL, so they get stripped anyway: a quote arriving
  # here would be a bug elsewhere and should not become an injection.
  defp escape(text), do: String.replace(text, ~r/[^0-9\-: .]/, "")
end
