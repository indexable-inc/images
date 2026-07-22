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
  """

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
    # `$0` is `cmd` and `$@` the args, so no shell parsing touches them.
    System.cmd(
      "/bin/sh",
      ["-c", ~S(exec "$0" "$@" </dev/null), cmd | args],
      Keyword.put_new(opts, :cd, launch_cwd())
    )
  end

  @doc """
  Run a one-line shell script (pipes, redirects) with stdin from `/dev/null`.

  The leading bare `exec` redirects the whole script, so pipeline heads
  (`rg pat | head`) see EOF too, not just the final command.
  """
  @spec sh(binary(), keyword()) :: {Collectable.t(), non_neg_integer()}
  def sh(script, opts \\ []) do
    System.cmd(
      "/bin/sh",
      ["-c", "exec </dev/null\n" <> script],
      Keyword.put_new(opts, :cd, launch_cwd())
    )
  end
end
