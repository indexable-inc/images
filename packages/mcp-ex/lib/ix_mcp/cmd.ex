defmodule IxMcp.Cmd do
  @moduledoc """
  `System.cmd/3` with a stdin that delivers EOF.

  A port's stdin is a pipe the BEAM never closes, so a subprocess that falls
  back to reading stdin when given no input path -- pathless `rg` and `grep`
  especially -- blocks forever (#3867). Wrapping the spawn in `sh` with stdin
  redirected from `/dev/null` hands every command an immediate EOF; `exec`
  replaces the shell, so no extra process outlives the redirect and job
  cancellation (`IxMcp.OsProc`) still sees one process tree.

  Commands run in the launch directory unless `cd:` says otherwise. The OS
  process cwd is BEAM-global, and any cell can move it with `File.cd!/1` --
  which is how one agent's pathless `git reset --hard` destroyed a sibling
  agent's worktree when several agents shared one kernel (#3902). `run/3`
  and `sh/2` therefore never consult the OS cwd at spawn time: the default
  `cd:` is the cwd captured once at application boot (`capture_launch_cwd/0`,
  called before any cell can run), immutable for the life of the instance.
  Working anywhere else takes an explicit `cd:` (or `git -C`).

  Git commands that would mutate a protected primary checkout are refused
  here (`IxMcp.GitGuard`, #4225), because the Bash-side `git-guard` hook
  never sees a kernel spawn.

  A `cd:` directory that does not exist raises before the spawn. Erlang's
  child setup reports a failed `chdir` by exiting with the raw errno --
  `{"", 2}` for ENOENT, `{"", 20}` for ENOTDIR -- indistinguishable from
  the command's own exit status, so a session whose launch dir was deleted
  mid-session saw every command "fail" with empty exit-2 output (#3979).
  """

  alias IxMcp.GitGuard
  alias IxMcp.Jobs

  defmodule Error do
    @moduledoc "A subprocess exited nonzero under `Cmd.run!/3` or `Cmd.sh!/2`."
    defexception [:message, :status, :output]
  end

  @launch_cwd_key {__MODULE__, :launch_cwd}

  @doc """
  Capture the OS cwd as this instance's immutable launch directory.

  Called from `IxMcp.Application.start/2` before the supervision tree, so
  the capture races no cell. `:persistent_term` rather than server state:
  no process restart can re-read a cwd a cell has since moved.
  """
  @spec capture_launch_cwd() :: :ok
  def capture_launch_cwd do
    :persistent_term.put(@launch_cwd_key, File.cwd!())
  end

  @doc "The directory pathless commands run in. Raises when boot never captured it."
  @spec launch_cwd() :: binary()
  def launch_cwd do
    :persistent_term.get(@launch_cwd_key)
  end

  @doc """
  Run `cmd` with `args`, stdin redirected from `/dev/null`.

  Options and return match `System.cmd/3` (`cd:`, `env:`,
  `stderr_to_stdout:`, `into:`, ...), except a missing executable exits 127
  with a shell diagnostic instead of raising, because `sh` resolves `cmd`.
  """
  @spec run(binary(), [binary()], keyword()) :: {Collectable.t(), non_neg_integer()}
  def run(cmd, args \\ [], opts \\ []) do
    opts = validate_cd!(opts)
    GitGuard.check!(cmd, args, opts[:cd])

    cmd
    |> run_unguarded(args, opts)
    |> note_nonzero(label(cmd, args))
  end

  @doc """
  `run/3` that raises `IxMcp.Cmd.Error` on a nonzero exit and returns the
  output alone on success. stderr is folded into the output unless the
  caller said otherwise, so the raise carries the command's own diagnostic.
  Reach for this when a failure should fail the cell -- the default `run/3`
  returns `{out, status}` and only attaches a note to the job.
  """
  @spec run!(binary(), [binary()], keyword()) :: Collectable.t()
  def run!(cmd, args \\ [], opts \\ []) do
    {out, status} = run(cmd, args, Keyword.put_new(opts, :stderr_to_stdout, true))
    raise_nonzero(label(cmd, args), out, status)
  end

  @doc false
  # The spawn without the git guard, for the guard's own `git rev-parse`
  # probes: a guard that can refuse the question it asks cannot answer it.
  @spec run_unguarded(binary(), [binary()], keyword()) :: {Collectable.t(), non_neg_integer()}
  def run_unguarded(cmd, args \\ [], opts \\ []) do
    opts = validate_cd!(opts)

    # `$0` is `cmd` and `$@` the args, so no shell parsing touches them.
    System.cmd("/bin/sh", ["-c", ~S(exec "$0" "$@" </dev/null), cmd | args], opts)
    |> guard_cd_race(opts[:cd])
  end

  @doc """
  Run a one-line shell script (pipes, redirects) with stdin from `/dev/null`.

  The leading bare `exec` redirects the whole script, so pipeline heads
  (`rg pat | head`) see EOF too, not just the final command.
  """
  @spec sh(binary(), keyword()) :: {Collectable.t(), non_neg_integer()}
  def sh(script, opts \\ []) do
    opts = validate_cd!(opts)
    GitGuard.check_script!(script, opts[:cd])

    System.cmd("/bin/sh", ["-c", "exec </dev/null\n" <> script], opts)
    |> guard_cd_race(opts[:cd])
    |> note_nonzero(label(script, []))
  end

  @doc "`sh/2` that raises `IxMcp.Cmd.Error` on a nonzero exit; returns output alone."
  @spec sh!(binary(), keyword()) :: Collectable.t()
  def sh!(script, opts \\ []) do
    {out, status} = sh(script, Keyword.put_new(opts, :stderr_to_stdout, true))
    raise_nonzero(label(script, []), out, status)
  end

  defp raise_nonzero(_label, out, 0), do: out

  defp raise_nonzero(label, out, status) do
    shown = if is_binary(out), do: String.slice(out, -2048, 2048), else: inspect(out)

    raise Error,
      message: "`#{label}` exited #{status}:\n#{shown}",
      status: status,
      output: out
  end

  # A nonzero exit must be part of the verdict the agent reads: `status:
  # done` on the exec reply says only that the cell returned, and the report
  # found whole classes of failed searches riding green replies because the
  # `{out, 2}` tuple was never looked at. When the caller is a cell, attach
  # the exit as a note on its job (best effort, a cast); anywhere else --
  # server internals, tests -- there is no job and nothing to attach to.
  defp note_nonzero({_out, 0} = result, _label), do: result

  defp note_nonzero({_out, status} = result, label) when is_integer(status) do
    with id when is_binary(id) <- Process.get(:ix_job_id),
         {:ok, pid} <- Jobs.lookup(id) do
      Jobs.Job.note(
        pid,
        "note: `#{label}` exited #{status} (the cell kept running; " <>
          "ignore if the nonzero exit was handled)"
      )
    else
      _not_in_a_cell -> :ok
    end

    result
  end

  defp note_nonzero(result, _label), do: result

  defp label(cmd, args) do
    [cmd | args]
    |> Enum.join(" ")
    |> String.replace(~r/\s+/, " ")
    |> String.slice(0, 80)
  end

  # Default `cd:` to the launch dir and refuse to spawn into a directory
  # that is not there: the port would otherwise report the child's failed
  # `chdir` as `{"", errno}` with no diagnostic (#3979).
  @spec validate_cd!(keyword()) :: keyword()
  defp validate_cd!(opts) do
    {cd, hint} =
      case Keyword.fetch(opts, :cd) do
        {:ok, cd} -> {cd, ""}
        :error -> {launch_cwd(), " (session launch dir deleted?)"}
      end

    case File.stat(cd) do
      {:ok, %File.Stat{type: :directory}} ->
        Keyword.put(opts, :cd, cd)

      {:ok, %File.Stat{type: other}} ->
        raise ArgumentError, "cd target #{cd} is not a directory (#{other})"

      {:error, :enoent} ->
        raise ArgumentError, "cd target #{cd} does not exist#{hint}"

      {:error, reason} ->
        raise ArgumentError, "cd target #{cd} is not usable: #{:file.format_error(reason)}"
    end
  end

  # The validate/spawn gap (TOCTOU): a `cd:` deleted after validation still
  # produces the bare-errno exit. A zero status means `chdir` succeeded, so
  # only a nonzero status with the directory now gone is ambiguous -- raise
  # rather than hand back a status that may be errno, not the command's.
  @spec guard_cd_race({Collectable.t(), non_neg_integer()}, binary()) ::
          {Collectable.t(), non_neg_integer()}
  defp guard_cd_race({_out, status} = result, cd) do
    if status == 0 or File.dir?(cd) do
      result
    else
      raise "cd target #{cd} no longer exists; exit #{status} may be the " <>
              "raw chdir errno rather than the command's own status"
    end
  end
end
