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

  A `cd:` that no longer exists -- the launch directory deleted mid-session,
  or a caller naming a removed worktree -- raises `IxMcp.Cmd.DeadCwdError`
  before the spawn. Left to `System.cmd/3`, the port child's failed `chdir`
  exits 2 with nothing on stdout, so every command appears to fail as
  `{"", 2}` with no diagnostics (#3979).
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
      put_live_cd!(opts)
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
      put_live_cd!(opts)
    )
  end

  # `System.cmd/3` with a dead `cd:` does not raise: the port child's chdir
  # fails after the fork, the child exits 2 with nothing on stdout, and the
  # caller reads `{"", 2}` as if the command itself ran and failed. That is
  # how a deleted launch directory silently broke every command mid-session
  # while `:os.cmd/1` (no chdir) kept working (#3979). Checking here turns
  # the dead directory into a named error; a concurrent delete can still
  # race the spawn, but the window shrinks from "any time since boot" to
  # one syscall.
  defp put_live_cd!(opts) do
    {cd, default?} =
      case Keyword.fetch(opts, :cd) do
        {:ok, cd} -> {cd, false}
        :error -> {launch_cwd(), true}
      end

    if !File.dir?(cd) do
      raise IxMcp.Cmd.DeadCwdError, path: cd, default?: default?
    end

    Keyword.put(opts, :cd, cd)
  end
end

defmodule IxMcp.Cmd.DeadCwdError do
  @moduledoc """
  The directory a command would run in does not exist.

  Raised by `IxMcp.Cmd.run/3` and `IxMcp.Cmd.sh/2` before the spawn, because
  `System.cmd/3` reports a failed port-child `chdir` as `{"", 2}` --
  indistinguishable from the command's own exit (#3979).
  """
  defexception [:path, :default?]

  @impl true
  def message(%{path: path, default?: true}) do
    "launch directory #{path} no longer exists: it was deleted after the " <>
      "kernel booted, so every pathless Cmd.run/Cmd.sh would spawn-fail as " <>
      ~S({"", 2}) <> " (#3979); pass cd: naming a live directory"
  end

  def message(%{path: path, default?: false}) do
    "cd: #{path} is not a directory, so the spawn would fail as " <>
      ~S({"", 2}) <> " masquerading as the command's own exit (#3979)"
  end
end
