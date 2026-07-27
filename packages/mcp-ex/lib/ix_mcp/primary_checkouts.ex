defmodule IxMcp.PrimaryCheckouts do
  @moduledoc """
  The protected primary-checkout policy, stated once for every kernel guard
  that reads it.

  A primary checkout is a long-lived clone every agent session on the machine
  shares. The claude-code wrapper bakes the glob list into its PreToolUse
  hooks (`packages/agent/policy/hook-runner.nix`) and the workstation profile
  exports the same list to the kernel's own environment, because hooks never
  see kernel calls: `IxMcp.Edit` enforces it for writes (#3871) and
  `IxMcp.GitGuard` for git commands (#4225).

  Order: an operator `CLAUDE_CODE_PRIMARY_CHECKOUTS` wins over the
  provisioned `IX_DEFAULT_PRIMARY_CHECKOUTS`; both are colon-separated globs
  and an empty value disables the guards. App env `:primary_checkouts` pins
  the list for tests, the same seam shape as `:actions_db`.
  """

  @doc "The configured globs; `[]` means no guard is installed."
  @spec globs() :: [binary()]
  def globs do
    case Application.get_env(:ix_mcp, :primary_checkouts) do
      nil ->
        (System.get_env("CLAUDE_CODE_PRIMARY_CHECKOUTS") ||
           System.get_env("IX_DEFAULT_PRIMARY_CHECKOUTS") || "")
        |> String.split(":", trim: true)

      globs ->
        globs
    end
  end

  @doc """
  True when kill switch `name` holds anything non-empty, matching the hook
  crate's `flag_set` so an operator's `=1` means the same thing to the kernel
  and to the hooks.
  """
  @spec flag_set?(binary()) :: boolean()
  def flag_set?(name) do
    case System.get_env(name) do
      nil -> false
      "" -> false
      _set -> true
    end
  end

  @doc """
  `path`'s repo toplevel when that toplevel is a protected primary checkout,
  `nil` when the path is fine: outside any git repo, inside a linked
  worktree, or in a toplevel no glob matches.

  A path that does not exist yet resolves through its nearest existing
  ancestor, exactly like the hook, so creating a file decides the same way
  as editing one.
  """
  @spec protected_toplevel(Path.t(), [binary()]) :: binary() | nil
  def protected_toplevel(path, globs) do
    dir = path |> Path.dirname() |> nearest_existing_dir()

    with {:ok, gitdir} <- rev_parse(dir, "--git-dir"),
         {:ok, common} <- rev_parse(dir, "--git-common-dir"),
         # Linked worktree: private git-dir differs from the shared common dir.
         true <- gitdir == common,
         {:ok, toplevel} <- rev_parse(dir, "--show-toplevel"),
         true <- Enum.any?(globs, &glob_match?(&1, toplevel)) do
      toplevel
    else
      _allowed -> nil
    end
  end

  @doc """
  Same question for a directory that is already known to exist, e.g. a
  command's working directory rather than a file it would write.
  """
  @spec protected_dir(Path.t(), [binary()]) :: binary() | nil
  def protected_dir(dir, globs), do: protected_toplevel(Path.join(dir, "."), globs)

  defp nearest_existing_dir("/"), do: "/"

  defp nearest_existing_dir(dir) do
    if File.dir?(dir), do: dir, else: dir |> Path.dirname() |> nearest_existing_dir()
  end

  # Only stdout is parsed, matching the hook: merging stderr in would let git
  # warning chatter corrupt the compared paths and fail the guard open.
  # Spawned without the git guard, both to avoid recursing through it and
  # because a guard that can refuse its own probe cannot decide anything.
  defp rev_parse(dir, what) do
    git = System.get_env("IX_GIT") || "git"

    case IxMcp.Cmd.run_unguarded(git, ["-C", dir, "rev-parse", "--path-format=absolute", what]) do
      {out, 0} ->
        case String.trim(out) do
          "" -> :error
          resolved -> {:ok, resolved}
        end

      _not_a_repo ->
        :error
    end
  end

  # Shell case-glob semantics, matching the hook's matcher: `*` crosses `/`
  # and `?` matches any one character, judged against the whole toplevel.
  # `[...]` character classes are NOT translated (they match literally); the
  # house patterns are slash-and-star only, and a divergence here fails
  # toward the hook still denying the native tools.
  @doc false
  @spec glob_match?(binary(), binary()) :: boolean()
  def glob_match?(pattern, string) do
    regex =
      pattern
      |> String.graphemes()
      |> Enum.map_join(fn
        "*" -> ".*"
        "?" -> "."
        ch -> Regex.escape(ch)
      end)

    Regex.match?(Regex.compile!("\\A" <> regex <> "\\z"), string)
  end
end
