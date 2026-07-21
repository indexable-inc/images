defmodule IxMcp.Issues do
  @moduledoc """
  Atomic issue pickup (#3880): `Issues.pickup/1` from any cell claims a
  GitHub issue before a session starts working it, so two agents hearing the
  same `source="issues"` announcement (#3877) cannot both take it. The
  arbiter is the shared actions.db every kernel instance on a host already
  opens (`IxMcp.ActionLog`): a UNIQUE(repo, number) insert either wins the
  claim or reads back who beat this session to it. A won claim mirrors
  itself to GitHub with `gh issue edit --add-assignee @me`, best effort --
  visibility off-host, never arbitration -- and `IxMcp.IssueWatch` announces
  it to every session on the host as an `event="picked_up"` channel
  notification.

  Known limit, by design: actions.db is per host, so claims race across
  machines, and GitHub assignment is not compare-and-set, so it stays a
  mirror. Revisit if the fleet needs cross-host claims.
  """

  alias IxMcp.ActionLog
  alias IxMcp.Cmd
  alias IxMcp.Session

  @default_repo "indexable-inc/index"

  @doc """
  Claim an issue: a bare number claims it on `#{@default_repo}`, a
  `"owner/repo#n"` string claims it anywhere. Returns `{:ok, detail}` when
  this session won -- work the issue -- or `{:error, "claimed by session
  <label> at <time>"}` when another session got there first: pick a
  different issue.

  Options exist for tests: `:gh` (the executable to mirror the claim with;
  nil skips), `:action_log` (the arbiter server), `:session_id` (the
  claiming sessions row).
  """
  @spec pickup(integer() | String.t(), keyword()) :: {:ok, String.t()} | {:error, String.t()}
  def pickup(ref, opts \\ [])

  def pickup(number, opts) when is_integer(number) and number > 0 do
    claim(@default_repo, number, opts)
  end

  def pickup(ref, opts) when is_binary(ref) do
    case Regex.run(~r{\A([\w.-]+/[\w.-]+)#(\d+)\z}, ref) do
      [_, repo, number] ->
        claim(repo, String.to_integer(number), opts)

      nil ->
        {:error, "unrecognized issue ref #{inspect(ref)}; pass an integer or \"owner/repo#n\""}
    end
  end

  defp claim(repo, number, opts) do
    log = Keyword.get(opts, :action_log, ActionLog)
    session_id = Keyword.get_lazy(opts, :session_id, fn -> Session.ids().session_id end)

    case ActionLog.claim_issue(repo, number, session_id, log) do
      {:ok, claim} ->
        {:ok, "claimed #{repo}##{number} at #{claim.claimed_at}#{assign(repo, number, opts)}"}

      {:error, winner} ->
        {:error, "claimed by session #{label(winner)} at #{winner.claimed_at}"}

      :disabled ->
        # No arbiter, no claim: pretending to win here is exactly the double
        # pickup this module exists to prevent.
        {:error, "action log disabled (#3539); no arbiter to claim through"}
    end
  end

  # The GitHub mirror is best effort on purpose: the claim is already won in
  # the arbiter, so a failed `gh` (offline, no auth, repo permissions) must
  # not look like a lost claim. The outcome rides in the success detail.
  defp assign(repo, number, opts) do
    case Keyword.get_lazy(opts, :gh, &default_gh/0) do
      nil ->
        "; gh not found, GitHub assignee not mirrored"

      gh ->
        args = ["issue", "edit", Integer.to_string(number), "--repo", repo]

        case Cmd.run(gh, args ++ ["--add-assignee", "@me"], stderr_to_stdout: true) do
          {_out, 0} -> "; assigned @me on GitHub"
          {out, _nonzero} -> "; GitHub assign failed (mirror only): #{String.slice(out, 0, 200)}"
        end
    end
  end

  defp label(%{session: nil, session_id: id}), do: "##{id || "?"}"
  defp label(%{session: name}), do: name

  defp default_gh do
    System.get_env("IX_MCP_GH") || System.find_executable("gh")
  end
end
