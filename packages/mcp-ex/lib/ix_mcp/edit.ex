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

  alias IxMcp.PrimaryCheckouts

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
  # (packages/claude-hooks `worktree-guard`) that denies native Edit/Write
  # under the configured primary-checkout globs. Kernel writes bypass hooks
  # entirely, so the same policy is enforced here at the blessed write seam,
  # with the same knobs and the same message: the globs and the protected
  # question both come from `IxMcp.PrimaryCheckouts`, which `IxMcp.GitGuard`
  # reads too, and `CLAUDE_CODE_DISABLE_WORKTREE_GUARD` is the kill switch.
  # A relative path resolves against this process's cwd (`Path.expand`); the
  # hook resolves against the tool payload's cwd, which for kernel writes is
  # the same server process.
  defp guard_primary_checkout!(path) do
    globs = PrimaryCheckouts.globs()

    if globs != [] and not PrimaryCheckouts.flag_set?("CLAUDE_CODE_DISABLE_WORKTREE_GUARD") do
      case PrimaryCheckouts.protected_toplevel(Path.expand(path), globs) do
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
