defmodule Loom.Ix do
  @moduledoc """
  The `ix` CLI as loom's only VM surface.

  Every lifecycle verb is a short-lived `ix` invocation; the long-lived
  child run streams through a port. The binary is resolved from the
  `:loom` application env key `:ix_bin` (default `"ix"`), which the test
  suite points at a recording fake, so the exact argv of every verb is
  assertable without a VM.

  Snapshot ids ride the CLI's scripting contract: a piped
  `ix snapshot <vm>` prints the bare snapshot id on stdout. It returns as
  soon as the snapshot is captured, so loom passes `--wait-durable` to
  block until the replication confirm lands - the very next verb is a
  restore of that id.

  A recent `ix new` also waits out the confirm on its own, so this is
  belt and braces: loom runs whatever `ix` is on PATH, which may predate
  that, and being explicit keeps the wait independent of the CLI's
  version.
  """

  @typedoc "One CLI invocation's failure: non-zero exit or missing binary."
  @type run_error :: {:exit, non_neg_integer(), String.t()} | {:enoent, String.t()}

  @uuid_re ~r/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/

  @spec bin() :: String.t()
  def bin, do: Application.get_env(:loom, :ix_bin, "ix")

  # Global argv prefix (e.g. ["--admin"]) and extra restore argv (e.g.
  # ["--on", "hil-compute-2"]). Same-node guest-to-guest data-plane
  # dials hairpin-fail today (the CLI dials the node's public address,
  # which a guest on that node cannot reach), so an in-guest control VM
  # must currently pin forks onto a DIFFERENT node. Both default empty;
  # the platform fix removes the need.
  @spec prefix() :: [String.t()]
  defp prefix, do: Application.get_env(:loom, :ix_prefix, [])

  @spec restore_extra() :: [String.t()]
  defp restore_extra, do: Application.get_env(:loom, :restore_args, [])

  @doc """
  Run one `ix` verb to completion; `{:ok, stdout}` on exit 0.

  stderr is folded into the captured output so a failure carries the
  CLI's own message.
  """
  @spec run([String.t()]) :: {:ok, String.t()} | {:error, run_error()}
  def run(args) when is_list(args) do
    case System.cmd(bin(), prefix() ++ args, stderr_to_stdout: true) do
      {out, 0} -> {:ok, String.trim(out)}
      {out, status} -> {:error, {:exit, status, String.trim(out)}}
    end
  rescue
    ErlangError -> {:error, {:enoent, bin()}}
  end

  @doc """
  Snapshot `vm` and return the snapshot id once it is restorable.

  The id is recovered by shape rather than by line position, so spinner
  or notice lines folded in from stderr cannot corrupt it.
  """
  @spec snapshot(String.t()) :: {:ok, String.t()} | {:error, run_error() | :no_snapshot_id}
  def snapshot(vm) do
    with {:ok, out} <- run(["snapshot", vm, "--wait-durable"]) do
      case Regex.scan(@uuid_re, out) do
        [] -> {:error, :no_snapshot_id}
        matches -> {:ok, matches |> List.last() |> hd()}
      end
    end
  end

  @doc "Restore `snapshot_id` into a new VM called `name`."
  @spec new_from_snapshot(String.t(), String.t()) :: {:ok, String.t()} | {:error, run_error()}
  def new_from_snapshot(snapshot_id, name) do
    run(["new", snapshot_id, "--name", name, "--no-shell" | restore_extra()])
  end

  @spec stop(String.t()) :: {:ok, String.t()} | {:error, run_error()}
  def stop(vm), do: run(["stop", vm])

  @spec start(String.t()) :: {:ok, String.t()} | {:error, run_error()}
  def start(vm), do: run(["start", vm])

  @spec rm(String.t()) :: {:ok, String.t()} | {:error, run_error()}
  def rm(vm), do: run(["rm", vm, "--force"])

  @doc """
  Stream `command` inside `vm` as a port owned by the calling process.

  Wraps `ix shell <vm> --noninteractive -- <command>`: the process runs
  in the guest, the port lives here. The caller receives standard port
  messages (`{port, {:data, {:eol, line}}}`, `{port, {:exit_status, n}}`).
  """
  @spec shell_stream(String.t(), [String.t()]) :: port()
  def shell_stream(vm, command) when is_list(command) do
    Port.open(
      {:spawn_executable, resolve_bin!()},
      [
        :binary,
        :exit_status,
        :hide,
        # stderr folded in: the CLI's own failures (attach refused, VM
        # not ready) must land in the agent's log, not vanish.
        :stderr_to_stdout,
        {:line, 1_048_576},
        {:args, prefix() ++ ["shell", vm, "--noninteractive", "--" | command]}
      ]
    )
  end

  # `Port.open/2 spawn_executable` needs a resolvable path, not a bare
  # name; a missing binary should fail loudly at spawn time.
  @spec resolve_bin!() :: charlist()
  defp resolve_bin! do
    name = bin()

    case System.find_executable(name) do
      nil -> raise ArgumentError, "ix binary not found on PATH: #{inspect(name)}"
      path -> String.to_charlist(path)
    end
  end
end
