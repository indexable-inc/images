defmodule IxMcp.Cmd do
  @moduledoc """
  `System.cmd/3` with a stdin that delivers EOF.

  A port's stdin is a pipe the BEAM never closes, so a subprocess that falls
  back to reading stdin when given no input path -- pathless `rg` and `grep`
  especially -- blocks forever (#3867). Wrapping the spawn in `sh` with stdin
  redirected from `/dev/null` hands every command an immediate EOF; `exec`
  replaces the shell, so no extra process outlives the redirect and job
  cancellation (`IxMcp.OsProc`) still sees one process tree.
  """

  @doc """
  Run `cmd` with `args`, stdin redirected from `/dev/null`.

  Options and return match `System.cmd/3` (`cd:`, `env:`,
  `stderr_to_stdout:`, `into:`, ...), except a missing executable exits 127
  with a shell diagnostic instead of raising, because `sh` resolves `cmd`.
  """
  @spec run(binary(), [binary()], keyword()) :: {Collectable.t(), non_neg_integer()}
  def run(cmd, args \\ [], opts \\ []) do
    # `$0` is `cmd` and `$@` the args, so no shell parsing touches them.
    System.cmd("/bin/sh", ["-c", ~S(exec "$0" "$@" </dev/null), cmd | args], opts)
  end

  @doc """
  Run a one-line shell script (pipes, redirects) with stdin from `/dev/null`.

  The leading bare `exec` redirects the whole script, so pipeline heads
  (`rg pat | head`) see EOF too, not just the final command.
  """
  @spec sh(binary(), keyword()) :: {Collectable.t(), non_neg_integer()}
  def sh(script, opts \\ []) do
    System.cmd("/bin/sh", ["-c", "exec </dev/null\n" <> script], opts)
  end
end
