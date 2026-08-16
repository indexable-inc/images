defmodule IxMcp.Forge.VerdictAnnounce do
  @moduledoc """
  The renderer for `IxMcp.Forge.Verdicts`: one channel line per terminal CI
  run.

  A verdict has to be readable in one glance and actionable without a
  follow-up query, which fixes what the line carries: the verdict word first,
  then the bookmark and the twelve-hex commit and change so a reader can
  match it against what they submitted, then how long the run took, and on a
  failure the stages that failed, the checks that were already red on the
  target tip, and the path to the full log.

  ## Two things the line deliberately does not claim

  The duration is RUN duration -- the record's `started_at_ms` is set after
  the run was dequeued, so time spent waiting in the queue is not in it and
  the line does not say "queued". A number whose denominator is unstated is
  the way a reader ends up quoting it as something else later.

  A truncated id is a prefix, not an identity, so the line says "commit" and
  "change" beside the twelve hex digits and the untruncated `run_id` rides in
  `meta.id`. Twelve is what the forge's own run directories and `jj log`
  short forms use, so the prefixes match what a reader sees elsewhere.

  ## Known wart, tracked separately

  `meta.source` here duplicates the `source` attribute the client's renderer
  injects for the MCP server itself, so a rendered block reads
  `source="index" ... source="forge"` and last-wins parsing takes ours. That
  is wrong and it is wrong identically for every other producer in this
  kernel (agents, jobs, fleet, issues, sessions, requests, and the inbox
  feeds), so this one matches them rather than becoming the single
  inconsistent case; see the note in `IxMcp.MCP.Notifier`. Counting this
  module as one more producer for that rename is the intended outcome.
  """

  @behaviour IxMcp.Inbox.Renderer

  alias IxMcp.MCP.Notifier

  @ms_per_second 1_000
  @seconds_per_minute 60
  @seconds_per_hour 3_600

  @typedoc """
  One terminal CI run, as `IxMcp.Forge.Verdicts` reads it out of the
  reconciler's run record.

    * `:id` - the run id (`<commit12>-<epoch_ms>`), unique per run and the
      dedup key. A change that is retried produces a second run with the
      same `:change_id` and a different `:id`, which is correct: both
      verdicts happened.
    * `:verdict` - `:pass` or `:fail`, the two terminal states.
    * `:change_id`, `:commit_id` - twelve-hex prefixes, or `"?"`.
    * `:target` - the bookmark the run was gating, normally `"main"`.
    * `:duration_ms` - run duration, or `nil` when the record cannot say.
    * `:failed_stages`, `:tolerated` - best-effort detail, empty when the
      log could not be read that way.
    * `:log` - path to the full gate log, or `nil`.
  """
  @type item :: %{
          id: String.t(),
          verdict: :pass | :fail,
          change_id: String.t(),
          commit_id: String.t(),
          target: String.t(),
          duration_ms: non_neg_integer() | nil,
          failed_stages: [String.t()],
          tolerated: [String.t()],
          log: String.t() | nil
        }

  @doc """
  Push one verdict onto the channel.

  Every meta value is a short string by construction: `Verdicts` normalizes
  the ids, and `Notifier.channel/2` raises on anything else.
  """
  @impl true
  @spec announce(String.t(), item()) :: :ok
  def announce(source, item) when is_binary(source) do
    Notifier.channel(line(source, item), %{
      "source" => source,
      "verdict" => verdict(item),
      "change" => item.change_id,
      "commit" => item.commit_id,
      "id" => item.id
    })
  end

  @doc """
  Say that a sweep found more terminal runs than its limit.

  Runs are announced newest-first-capped, so the ones a cap drops are the
  OLDEST unheard verdicts -- the ones a reader is least likely to go looking
  for and most likely to have been waiting on.
  """
  @impl true
  @spec announce_overflow(String.t(), pos_integer()) :: :ok
  def announce_overflow(source, shown) when is_binary(source) and is_integer(shown) do
    Notifier.channel(
      "#{source}: more CI runs reached a verdict than this sweep's limit of #{shown}; " <>
        "the oldest of them were not announced",
      %{"source" => source, "overflow" => "true"}
    )
  end

  @doc """
  A duration as a short human string: `"48s"`, `"5m9s"`, `"1h4m"`.

  `nil` renders as a marker rather than a zero, because a run that took no
  measurable time is a different claim from a run whose record did not say.
  """
  @spec duration(non_neg_integer() | nil) :: String.t()
  def duration(ms) when is_integer(ms) and ms >= 0 do
    seconds = div(ms, @ms_per_second)

    cond do
      seconds >= @seconds_per_hour ->
        "#{div(seconds, @seconds_per_hour)}h#{div(rem(seconds, @seconds_per_hour), @seconds_per_minute)}m"

      seconds >= @seconds_per_minute ->
        "#{div(seconds, @seconds_per_minute)}m#{rem(seconds, @seconds_per_minute)}s"

      true ->
        "#{seconds}s"
    end
  end

  def duration(_unknown), do: "unknown time"

  @spec line(String.t(), item()) :: String.t()
  defp line(source, item) do
    "#{source} CI #{String.upcase(verdict(item))} #{item.target} #{item.commit_id} " <>
      "(change #{item.change_id}) in #{duration(item.duration_ms)}" <> detail(item)
  end

  # The garnish, each part omitted when the log did not carry it, so a FAIL
  # whose detail could not be parsed still reads as a complete sentence.
  defp detail(%{verdict: :pass}), do: ""

  defp detail(item) do
    [
      names("failed", item.failed_stages),
      names("already red on target", item.tolerated),
      path(item.log)
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.map_join(&"; #{&1}")
  end

  defp names(_label, []), do: nil
  defp names(label, names), do: "#{label}: #{Enum.join(names, ", ")}"

  defp path(nil), do: nil
  defp path(log), do: "log: #{log}"

  defp verdict(%{verdict: :pass}), do: "pass"
  defp verdict(_fail), do: "fail"
end
