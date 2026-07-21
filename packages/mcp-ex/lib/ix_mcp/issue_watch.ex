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
  """

  use GenServer

  require Logger

  alias IxMcp.Cmd
  alias IxMcp.MCP.Notifier

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
        state = %{
          gh: gh,
          owners: owners,
          interval_ms: Keyword.get(opts, :interval_ms, @interval_ms),
          since: DateTime.utc_now(:second),
          seen: MapSet.new()
        }

        {:ok, schedule(state)}
    end
  end

  @impl true
  def handle_info(:poll, state) do
    {:noreply, state |> sweep() |> schedule()}
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
