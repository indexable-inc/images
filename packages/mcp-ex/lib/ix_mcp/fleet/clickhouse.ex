defmodule IxMcp.Fleet.ClickHouse do
  @moduledoc """
  Read-only access to the fleet's ClickHouse, which is the only place fleet
  telemetry exists (hosts ship their journals there and keep nothing local).

  Transport is `ssh` to the cluster leader, not HTTP. Port 8123 is firewalled
  to the leader's vRack bond, so the machine this kernel runs on has no route
  to it -- a direct `curl http://clickhouse.ix.internal:8123` fails to resolve
  before it fails to connect. The leader can always reach its own ClickHouse,
  so the query runs there and only the rows come back. This also avoids adding
  an HTTP client dependency: `mcp-ex` deliberately carries none (mix.exs), and
  a fleet read is not worth changing that.

  Every function distinguishes an empty answer from an unanswered question
  (ENG-11209). `{:ok, []}` means the fleet is quiet; `{:error, reason}` means
  we could not look. Callers must not collapse the two -- a query that fails
  and reads as health is how a real incident stays invisible, and this exact
  confusion is why `fleet.unit_health_latest.unhealthy` went unexamined for
  weeks while it was structurally incapable of firing (ENG-11211).
  """

  @default_host "hil-compute-2"

  # ssh + query, generously. A leader under memory pressure answers slowly
  # rather than not at all, and a poll that gives up early reports "I could
  # not look" about a fleet that was merely busy.
  @connect_timeout_s 10
  @query_timeout_ms 60_000

  @typedoc "One result row: column name to decoded JSON value."
  @type row :: %{String.t() => term()}

  @doc """
  The leader to query. `IX_CLICKHOUSE_HOST` overrides; it is an ssh
  destination, so an alias from `~/.ssh/config` is as valid as a hostname.
  Leadership moves, and when it does this is the one line to change.
  """
  @spec host() :: String.t()
  def host, do: System.get_env("IX_CLICKHOUSE_HOST") || @default_host

  @doc """
  Run `sql` on the leader and decode `JSONEachRow` output.

  Returns `{:ok, rows}` or `{:error, reason}` where `reason` is a short
  human-readable string naming what failed -- unreachable host, ClickHouse
  error, or undecodable output. The reason is meant to be shown to an
  operator verbatim, so it names the host and trims ClickHouse's stack noise.
  """
  @spec query(String.t()) :: {:ok, [row()]} | {:error, String.t()}
  def query(sql) when is_binary(sql) do
    host = host()

    # ssh does not take an argv the way System.cmd does: it joins everything
    # after the destination with spaces and hands the result to the remote
    # user's shell. So the query must arrive as ONE shell word, single-quoted,
    # or the remote bash word-splits it and clickhouse sees only "SELECT".
    # Passing it as a separate list element looks right and is not -- it cost
    # a debugging round to find, because the symptom was a ClickHouse syntax
    # error at position 7 rather than anything mentioning ssh.
    remote = "clickhouse-client --format JSONEachRow --query " <> shell_quote(sql)

    args = [
      "-o",
      "BatchMode=yes",
      "-o",
      "ConnectTimeout=#{@connect_timeout_s}",
      host,
      remote
    ]

    case run(args) do
      {output, 0} -> decode(output, host)
      {output, status} -> {:error, "#{host}: clickhouse exited #{status}: #{trim(output)}"}
    end
  catch
    :exit, reason -> {:error, "#{host()}: ssh did not run: #{inspect(reason)}"}
  end

  # stderr is folded into stdout so a ClickHouse diagnostic survives into the
  # error reason instead of vanishing. A hung ssh is bounded by the task
  # timeout: without it a poller blocks forever on a host that accepts the
  # connection and then says nothing.
  defp run(args) do
    task = Task.async(fn -> System.cmd("ssh", args, stderr_to_stdout: true) end)

    case Task.yield(task, @query_timeout_ms) || Task.shutdown(task, :brutal_kill) do
      {:ok, result} -> result
      nil -> {"timed out after #{div(@query_timeout_ms, 1000)}s", 124}
    end
  end

  defp decode(output, host) do
    output
    |> String.split("\n", trim: true)
    |> Enum.reduce_while({:ok, []}, fn line, {:ok, rows} ->
      case JSON.decode(line) do
        {:ok, row} when is_map(row) -> {:cont, {:ok, [row | rows]}}
        _ -> {:halt, {:error, "#{host}: undecodable row: #{trim(line)}"}}
      end
    end)
    |> case do
      {:ok, rows} -> {:ok, Enum.reverse(rows)}
      error -> error
    end
  end

  # POSIX single-quote quoting: everything is literal inside '...', and the
  # only character that cannot appear there is ' itself, which is emitted by
  # closing the quote, escaping one bare quote, and reopening. Our SQL is full
  # of string literals ('alert', 'emerg'), so this is load-bearing, not
  # defensive.
  defp shell_quote(text), do: "'" <> String.replace(text, "'", "'\\''") <> "'"

  defp trim(text), do: text |> String.trim() |> String.slice(0, 300)
end
