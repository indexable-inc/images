defmodule IxMcp.GitGuard do
  @moduledoc """
  Refuses git commands that would mutate a protected primary checkout
  (#4225).

  `git-guard` already refuses these through Claude Code's Bash tool
  (index#4218), but the kernel's `IxMcp.Cmd` never passes through hooks, so
  `Cmd.sh("git add -A", cd: <protected>)` from an exec cell reached the
  shared checkout unrefused. This is the same gap `IxMcp.Edit` closed for
  writes (#3871), fixed the same way: the policy is enforced at the kernel's
  own spawn seam.

  The mutating set is closed and enumerated, ported from `mutates_checkout`
  in `packages/claude-hooks/src/guards.rs`, so an unclassified subcommand
  allows rather than refuses. `test/ix_mcp/git_guard_drift_test.exs` fails
  when the two sets diverge.

  Kill switch: `CLAUDE_CODE_DISABLE_GIT_GUARD`, the hook's own name, so one
  export covers both enforcement points.
  """

  alias IxMcp.PrimaryCheckouts

  @kill_switch "CLAUDE_CODE_DISABLE_GIT_GUARD"

  # Subcommand => why it mutates. Ported from the hook crate; the drift test
  # compares the key set, not the prose.
  @mutating %{
    "add" => "stages changes into the index that every session here shares",
    "am" => "applies a mailbox of patches onto the current branch",
    "apply" => "writes a patch into the working tree or the index",
    "checkout" => "moves HEAD, or overwrites paths in the working tree",
    "cherry-pick" => "commits another branch's change onto the current one",
    "clean" => "deletes untracked files",
    "commit" => "writes a commit out of the index that every session here shares",
    "merge" => "merges into the current branch",
    "mv" => "renames tracked paths",
    "pull" => "fetches, then merges or rebases the current branch",
    "rebase" => "rewrites the current branch",
    "reset" => "moves HEAD, or rewrites the index",
    "restore" => "restores paths over the index or the working tree",
    "revert" => "commits the inverse of a change onto the current branch",
    "rm" => "deletes tracked paths",
    "stash" => "moves uncommitted work off the working tree",
    "switch" => "switches the checkout to another branch"
  }

  # `apply` only reports with these; `stash` only reads with these. Both
  # match the hook's carve-outs, which exist because the refusal text itself
  # recommends `git stash list`.
  @apply_read_only ["--check", "--stat", "--numstat", "--summary"]
  @stash_read_only ["list", "show", "create"]

  @doc "The mutating subcommands, for the drift test."
  @spec mutating_subcommands() :: [binary()]
  def mutating_subcommands, do: @mutating |> Map.keys() |> Enum.sort()

  @doc """
  Raise when `cmd`/`args` run from `cd` would mutate a protected checkout.

  Everything else returns `:ok`, including any command that is not git, any
  subcommand outside the closed set, and every read.
  """
  @spec check!(binary(), [binary()], Path.t()) :: :ok
  def check!(cmd, args, cd), do: check!(cmd, args, cd, [])

  @doc """
  The same check, given the child's `env` as well.

  `GIT_DIR`/`GIT_WORK_TREE` aim git somewhere other than its cwd, so a guard
  that never sees the env can be stepped around by exporting one of them:
  `git add -A` with `env: [{"GIT_DIR", protected}]` was measured ALLOWED against
  a protected checkout while the plain form was refused.
  """
  @spec check!(binary(), [binary()], Path.t(), [{binary(), binary() | false}]) :: :ok
  def check!(cmd, args, cd, env) do
    with false <- PrimaryCheckouts.flag_set?(@kill_switch),
         globs when globs != [] <- PrimaryCheckouts.globs(),
         true <- git?(cmd),
         {sub, rest} <- split_subcommand(args),
         {:ok, reason} <- mutation(sub, rest),
         target when is_binary(target) <-
           PrimaryCheckouts.protected_dir(target_dir(args, cd, env), globs) do
      raise ArgumentError, refusal(sub, reason, target)
    else
      _allowed -> :ok
    end
  end

  @doc """
  The same check for a shell script, applied to each command position a
  simple reader can see: the start of the script and whatever follows `;`,
  `&&`, `||`, `|` or a newline.

  Quoting, substitution and here-docs are not parsed, so a git call hidden
  inside them is missed. That is the fail-open direction on purpose: a
  script the reader cannot understand must not become a refusal.
  """
  @spec check_script!(binary(), Path.t()) :: :ok
  def check_script!(script, cd) do
    script
    |> String.split(~r/(\n|;|&&|\|\||\|)/, trim: true)
    |> Enum.each(fn segment ->
      case String.split(segment, ~r/\s+/, trim: true) do
        [cmd | args] -> check!(cmd, args, cd)
        [] -> :ok
      end
    end)
  end

  defp git?(cmd), do: Path.basename(cmd) == "git"

  # The subcommand is the first argument that is not a global option. The flags
  # below take a value, so their argument is skipped rather than mistaken for the
  # subcommand -- `git --git-dir P/.git add -A` otherwise read "P/.git" as the
  # subcommand, found it outside the mutating set, and allowed a staging command
  # into a protected checkout.
  @valued_globals ["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--exec-path"]

  defp split_subcommand(args), do: split_subcommand(args, [])

  defp split_subcommand([], _seen), do: :no_subcommand

  defp split_subcommand([flag, _value | rest], seen) when flag in @valued_globals,
    do: split_subcommand(rest, seen)

  defp split_subcommand([<<"-", _::binary>> | rest], seen), do: split_subcommand(rest, seen)

  defp split_subcommand([sub | rest], _seen), do: {sub, rest}

  defp mutation(sub, args) do
    if read_only_form?(sub, args) do
      :allowed
    else
      case Map.fetch(@mutating, sub) do
        {:ok, reason} -> {:ok, reason}
        :error -> :allowed
      end
    end
  end

  # The forms of a mutating subcommand that only report, matching the hook's
  # carve-outs. `--dry-run` is git's universal "print what I would do": every
  # subcommand above that accepts it honors it, and the ones that do not
  # reject it and mutate nothing either way. `-n` is a dry run only where the
  # subcommand says so, since on `commit` it means `--no-verify`.
  defp read_only_form?(sub, args) do
    cond do
      "--dry-run" in args -> true
      sub == "apply" -> Enum.any?(@apply_read_only, &(&1 in args))
      sub == "stash" -> first_operand(args) in @stash_read_only
      sub in ["mv", "rm"] -> short_flag?(args, "n")
      # `clean` deletes only when forced, and `-n` there is the dry run.
      sub == "clean" -> not forced?(args) or short_flag?(args, "n")
      true -> false
    end
  end

  defp forced?(args), do: "--force" in args or short_flag?(args, "f")

  # A bundled short flag: `-fdn` sets f, d and n.
  defp short_flag?(args, letter) do
    Enum.any?(args, fn
      "--" <> _long -> false
      "-" <> letters -> String.contains?(letters, letter)
      _operand -> false
    end)
  end

  defp first_operand(args), do: Enum.find(args, &(not String.starts_with?(&1, "-")))

  # `git -C <dir>` aims the command at `<dir>`, whatever the spawn's cwd is.
  # A relative `-C` resolves against that cwd, the way git resolves it.
  #
  # `--work-tree`/`--git-dir` and their GIT_WORK_TREE/GIT_DIR env twins aim it
  # somewhere else again, and reading only `-C` meant
  # `git --git-dir P/.git --work-tree P add -A` reached a protected checkout
  # unrefused. Precedence follows git's own: an explicit flag beats the
  # environment, a work tree beats a git dir, and a bare git dir stands for its
  # parent (P/.git -> P), which is the checkout a mutation would land in.
  defp target_dir(args, cd, env) do
    from_c = Path.expand(valued_global(args, "-C") || ".", cd)
    work_tree = valued_global(args, "--work-tree") || env_value(env, "GIT_WORK_TREE")
    git_dir = valued_global(args, "--git-dir") || env_value(env, "GIT_DIR")

    cond do
      is_binary(work_tree) -> Path.expand(work_tree, from_c)
      is_binary(git_dir) -> git_dir |> Path.expand(from_c) |> strip_git_dir()
      true -> from_c
    end
  end

  defp strip_git_dir(dir) do
    if Path.basename(dir) == ".git", do: Path.dirname(dir), else: dir
  end

  # Both spellings: `--git-dir P` and `--git-dir=P`.
  defp valued_global(args, flag) do
    prefix = flag <> "="

    case Enum.find(args, &String.starts_with?(&1, prefix)) do
      nil -> separate_value(args, flag)
      arg -> String.replace_prefix(arg, prefix, "")
    end
  end

  defp separate_value(args, flag) do
    args
    |> Enum.chunk_every(2, 1, :discard)
    |> Enum.find_value(fn
      [^flag, value] -> value
      _other -> nil
    end)
  end

  defp env_value(env, name) do
    Enum.find_value(env, fn
      {^name, value} when is_binary(value) -> value
      _other -> nil
    end)
  end

  defp refusal(sub, reason, target) do
    """
    Refusing `git #{sub}` in #{target}.

    #{target} is a protected primary checkout, shared by every agent session \
    on this machine. `git #{sub}` #{reason}. Never work in a primary \
    checkout: one was found on a branch deleted upstream, 604 commits behind \
    main, with 534 files staged by nobody, reached entirely through `git add` \
    and `git switch` (index#4218, index#4225).

    Work in a worktree of your own instead:
        git -C #{target} worktree add /tmp/worktree/<org>/<repo>/<name> -b <branch> origin/main
        git -C /tmp/worktree/<org>/<repo>/<name> submodule update --init --recursive

    Reading here is always fine: status, log, diff, show, ls-files, rev-parse, \
    branch, fetch, worktree list/add, stash list, and anything with --dry-run.

    (kernel git guard, index#4225; kill switch #{@kill_switch}=1)
    """
  end
end
