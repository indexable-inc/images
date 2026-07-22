defmodule IxMcp.IssueWatch do
  @moduledoc """
  Always-on feed of newly filed GitHub issues, announced on the channel the
  way job finishes are. Background agents (retros, auto-fix dispatches) file
  issues from sessions nobody watches, so without a push they sit unseen
  until someone happens to run `gh issue list` (#3877). One loop per kernel
  instance polls `gh search issues` for issues created since the last sweep
  and pushes each new one as a `source="issues"` channel notification.

  Watched owners come from `IX_MCP_ISSUE_WATCH_OWNERS` (comma-separated,
  default `indexable-inc`); an empty value disables the feed. The loop
  starts only alongside the stdio transport (`IxMcp.Application`), so
  `mix test` and IEx sessions never poll GitHub.

  The same sweep also announces issue pickups (#3880): claims sessions win
  through `IxMcp.Issues.pickup/1` land in the shared actions.db, and each
  sweep pushes the ones this instance has not yet announced as
  `event="picked_up"` notifications. The cursor is a per-instance claim-id
  watermark (starting at the newest claim on boot), NOT a shared announced
  flag: every instance on the host must tell its own client, and a shared
  flag would let the first sweeper silence all the others.
  """

  use GenServer

  alias IxMcp.ActionLog
  alias IxMcp.Cmd
  alias IxMcp.MCP.Notifier

  require Logger

  @default_owners ["indexable-inc"]
  @interval_ms 60_000
  # One page bounds a sweep; more issues than this filed inside one interval
  # is a storm, and the next sweep's >= window picks up the remainder anyway.
  @page_limit 50

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, __MODULE__))
  end

  @impl true
  def init(opts) do
    owners = Keyword.get(opts, :owners, env_owners())
    gh = Keyword.get(opts, :gh, default_gh())

    cond do
      owners == [] ->
        :ignore

      gh == nil ->
        Logger.error("IssueWatch disabled: gh not found (no IX_MCP_GH, none on PATH)")
        :ignore

      true ->
        action_log = Keyword.get(opts, :action_log, ActionLog)

        state = %{
          gh: gh,
          owners: owners,
          interval_ms: Keyword.get(opts, :interval_ms, @interval_ms),
          since: DateTime.utc_now(:second),
          seen: MapSet.new(),
          action_log: action_log,
          # Claims standing before this instance booted are old news; only
          # pickups from here on announce (the filed-issue feed's since: now).
          claims_since: ActionLog.last_issue_claim_id(action_log)
        }

        {:ok, schedule(state)}
    end
  end

  @impl true
  def handle_info(:poll, state) do
    {:noreply, state |> sweep() |> sweep_claims() |> schedule()}
  end

  defp schedule(state) do
    Process.send_after(self(), :poll, state.interval_ms)
    state
  end

  defp sweep(state) do
    case search(state) do
      {:ok, issues} ->
        issues
        |> Enum.reject(fn issue -> MapSet.member?(state.seen, issue["url"]) end)
        |> Enum.each(&announce/1)

        advance(state, issues)

      {:error, detail} ->
        # A failed sweep resolves itself next tick (gh hiccup, rate limit,
        # network); log it rather than turn the channel into a heartbeat.
        Logger.warning("IssueWatch sweep failed: #{detail}")
        state
    end
  end

  # created:>= re-reads the boundary second, so `seen` (the previous sweep's
  # URLs) is what keeps a same-second issue from announcing twice.
  defp search(state) do
    args =
      ["search", "issues", "--sort", "created", "--order", "asc"] ++
        ["--limit", Integer.to_string(@page_limit)] ++
        ["--created", ">=" <> DateTime.to_iso8601(state.since)] ++
        ["--json", "number,title,url,repository,author,createdAt"] ++
        Enum.flat_map(state.owners, fn owner -> ["--owner", owner] end)

    case Cmd.run(state.gh, args, stderr_to_stdout: true) do
      {out, 0} ->
        case JSON.decode(out) do
          {:ok, issues} when is_list(issues) -> {:ok, issues}
          _ -> {:error, "unparseable gh output: #{String.slice(out, 0, 200)}"}
        end

      {out, _nonzero} ->
        {:error, String.slice(out, 0, 400)}
    end
  end

  # Pickup fan-out (#3880): read past the watermark, announce, advance. The
  # claim rows come from the shared database, so this hears every session on
  # the host, including this instance's own pickups (harmless: the picker
  # already knows, and one notification tells its transcript too).
  defp sweep_claims(state) do
    case ActionLog.issue_claims_after(state.claims_since, state.action_log) do
      [] ->
        state

      claims ->
        Enum.each(claims, &announce_claim/1)
        %{state | claims_since: claims |> Enum.map(& &1.id) |> Enum.max()}
    end
  end

  defp announce_claim(claim) do
    ref = "#{claim.repo}##{claim.number}"
    label = claim.session || "##{claim.session_id || "?"}"

    Notifier.channel(
      "issue picked up: #{ref} by session #{label} at #{claim.claimed_at}",
      %{
        "source" => "issues",
        "event" => "picked_up",
        "issue" => ref,
        "session" => label,
        "level" => "info"
      }
    )
  end

  defp announce(issue) do
    repo = get_in(issue, ["repository", "nameWithOwner"]) || "?"
    author = get_in(issue, ["author", "login"]) || "?"
    ref = "#{repo}##{issue["number"]}"

    Notifier.channel(
      "issue filed: #{ref} by #{author}: #{issue["title"]}\n#{issue["url"]}",
      %{"source" => "issues", "issue" => ref, "author" => author, "level" => "info"}
    )
  end

  defp advance(state, []), do: state

  defp advance(state, issues) do
    since =
      issues
      |> Enum.map(fn issue ->
        {:ok, created, _offset} = DateTime.from_iso8601(issue["createdAt"])
        created
      end)
      |> Enum.max(DateTime)

    %{state | since: since, seen: MapSet.new(issues, fn issue -> issue["url"] end)}
  end

  defp env_owners do
    case System.get_env("IX_MCP_ISSUE_WATCH_OWNERS") do
      nil -> @default_owners
      value -> value |> String.split(",", trim: true) |> Enum.map(&String.trim/1)
    end
  end

  defp default_gh do
    System.get_env("IX_MCP_GH") || System.find_executable("gh")
  end
end
