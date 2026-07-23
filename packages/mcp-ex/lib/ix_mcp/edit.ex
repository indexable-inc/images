defmodule IxMcp.Edit do
  @moduledoc """
  File writing and exact-string find/replace for cells, aliased in the
  workspace prelude as `Edit`. The semantics mirror Claude Code's native
  Write and Edit tools (messages lifted from the 2.1.215 binary) because
  they are shaped for LLM callers: a replacement that matches nowhere or
  more than once fails loudly instead of guessing, `replace_all:` is an
  explicit opt-in, and success returns a line-numbered snippet of the
  edited region so the caller sees the change without re-reading. The
  native stale-read guard is subsumed by exactness: content that drifted
  since `old_string` was copied no longer matches, so the call raises
  instead of clobbering.

  Writes honor the primary-checkout worktree guard (#3871): the same
  denylist the claude-code PreToolUse hook enforces for the native Edit and
  Write tools applies here, because kernel writes never pass through hooks.
  See `guard_primary_checkout!/1`.
  """

  alias IxMcp.Cmd

  # Lines of context around the edited region in the success snippet.
  @snippet_context 4

  @doc """
  Replace one exact occurrence of `old_string` in `path` with `new_string`
  (every occurrence with `replace_all: true`).

  Raises `ArgumentError` when the strings are equal, `old_string` is empty,
  no occurrence exists, or several do without `replace_all: true`.
  """
  @spec replace(Path.t(), String.t(), String.t(), keyword()) :: String.t()
  def replace(path, old_string, new_string, opts \\ []) do
    guard_primary_checkout!(path)
    replace_all = Keyword.get(opts, :replace_all, false)
    validate_strings!(old_string, new_string)

    content = File.read!(path)
    matches = length(String.split(content, old_string)) - 1

    cond do
      matches == 0 ->
        raise ArgumentError,
              "String to replace not found in file. " <>
                "Re-read the file and copy the exact surrounding text."

      matches > 1 and not replace_all ->
        raise ArgumentError,
              "Found #{matches} matches of the string to replace, but replace_all is " <>
                "false. To replace all occurrences, set replace_all to true. To " <>
                "replace only one occurrence, please provide more context to " <>
                "uniquely identify the instance."

      true ->
        updated = String.replace(content, old_string, new_string, global: replace_all)
        File.write!(path, updated)
        note = snippet(content, updated, old_string, new_string)
        confirmation(path, matches, replace_all) <> "\n" <> note
    end
  end

  @doc """
  Write `content` to `path`, overwriting any existing file; parent
  directories are created. The return names whether the file was created
  or updated.
  """
  @spec write(Path.t(), String.t()) :: String.t()
  def write(path, content) do
    guard_primary_checkout!(path)
    existed = File.exists?(path)
    File.mkdir_p!(Path.dirname(path))
    File.write!(path, content)

    if existed do
      "The file #{path} has been updated successfully."
    else
      "File created successfully at: #{path}"
    end
  end

  # -- primary-checkout worktree guard (#3871) --------------------------------

  # The claude-code home-manager module ships a PreToolUse hook
  # (packages/claude-hooks `worktree-guard`) that denies native
  # Edit/Write under the configured primary-checkout globs. Kernel writes
  # bypass hooks entirely, so the same policy is enforced here at the
  # blessed write seam, with the same knobs and the same message: an
  # operator `CLAUDE_CODE_PRIMARY_CHECKOUTS` wins over the provisioned
  # `IX_DEFAULT_PRIMARY_CHECKOUTS` (colon-separated globs, empty disables),
  # `CLAUDE_CODE_DISABLE_WORKTREE_GUARD` is the kill switch, and a linked
  # worktree is always allowed. App env `:primary_checkouts` pins the globs
  # for tests, the same seam shape as `:actions_db`.
  # A relative path resolves against this process's cwd (`Path.expand`);
  # the hook resolves against the tool payload's cwd, which for kernel
  # writes is the same server process.
  defp guard_primary_checkout!(path) do
    globs = primary_checkouts()

    if globs != [] and not guard_disabled?() do
      case protected_toplevel(Path.expand(path), globs) do
        nil ->
          :ok

        toplevel ->
          raise ArgumentError,
                "Refusing to edit #{toplevel}: it is a primary checkout, not a worktree, " <>
                  "and other work may be in flight there. Create a dedicated worktree " <>
                  "(`git -C #{toplevel} worktree add <dir> -b <branch> origin/main`) and " <>
                  "edit the file there instead. Reads are always fine."
      end
    end

    :ok
  end

  defp guard_disabled? do
    case System.get_env("CLAUDE_CODE_DISABLE_WORKTREE_GUARD") do
      nil -> false
      "" -> false
      _set -> true
    end
  end

  defp primary_checkouts do
    case Application.get_env(:ix_mcp, :primary_checkouts) do
      nil ->
        (System.get_env("CLAUDE_CODE_PRIMARY_CHECKOUTS") ||
           System.get_env("IX_DEFAULT_PRIMARY_CHECKOUTS") || "")
        |> String.split(":", trim: true)

      globs ->
        globs
    end
  end

  # The target's repo toplevel when it is a protected primary checkout, nil
  # when the write is fine (outside any git repo, in a linked worktree, or
  # in a toplevel no glob matches). A new file's parent may not exist yet,
  # so the nearest existing ancestor's repo decides, exactly like the hook.
  defp protected_toplevel(target, globs) do
    dir = target |> Path.dirname() |> nearest_existing_dir()

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

  defp nearest_existing_dir("/"), do: "/"

  defp nearest_existing_dir(dir) do
    if File.dir?(dir), do: dir, else: dir |> Path.dirname() |> nearest_existing_dir()
  end

  # Only stdout is parsed, matching the hook: merging stderr in would let
  # git warning chatter corrupt the compared paths and fail the guard open.
  defp rev_parse(dir, what) do
    git = System.get_env("IX_GIT") || "git"

    case Cmd.run(git, ["-C", dir, "rev-parse", "--path-format=absolute", what]) do
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
  # `[...]` character classes are NOT translated (they match literally);
  # the house patterns are slash-and-star only, and a divergence here fails
  # toward the hook still denying the native tools.
  defp glob_match?(pattern, string) do
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

  defp validate_strings!(old_string, new_string) do
    if old_string == new_string do
      raise ArgumentError,
            "No changes to make: old_string and new_string are exactly the same."
    end

    if old_string == "" do
      raise ArgumentError,
            "old_string must not be empty; use Edit.write/2 to create content."
    end
  end

  defp confirmation(path, matches, replace_all) do
    if replace_all and matches > 1 do
      "The file #{path} has been updated. All occurrences were successfully replaced."
    else
      "The file #{path} has been updated successfully."
    end
  end

  # The edited region of `updated`, cat -n numbered with context lines,
  # anchored at the first occurrence of `old_string` in the original.
  defp snippet(original, updated, old_string, new_string) do
    {pos, _len} = :binary.match(original, old_string)
    first = newlines(binary_part(original, 0, pos))
    lines = String.split(updated, "\n")
    lo = max(first - @snippet_context, 0)
    hi = min(first + newlines(new_string) + @snippet_context, length(lines) - 1)

    lines
    |> Enum.slice(lo..hi//1)
    |> Enum.with_index(lo + 1)
    |> Enum.map_join("\n", fn {text, n} ->
      String.pad_leading(Integer.to_string(n), 6) <> "\t" <> text
    end)
  end

  defp newlines(s), do: length(String.split(s, "\n")) - 1
end
