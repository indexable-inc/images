defmodule Loom.Guard do
  @moduledoc """
  Resume-triggered fork detection.

  A snapshot fork of the control VM is a WARM restore: the parent's
  processes - including this BEAM, possibly mid-spawn - resume inside
  the fork. A boot-time guard never runs there (nothing boots), so the
  check has to live on the spawn path itself: record this VM's network
  identity at startup, and re-check it after every operation that can
  produce a fork. The fork gets a different address by construction
  (the guest's private IP moves with the VM index), so a changed
  identity means "this process is the clone" - and the clone must halt
  before it continues a spawn flow it never initiated, with the
  inherited token, against the real account. Without this, a resumed
  clone re-runs `new_from_snapshot` and every fork forks again.

  The identity probe is configurable (`:loom`, `:identity_cmd`) so
  tests pin it to a constant.
  """

  use Agent

  @spec start_link(term()) :: {:ok, pid()} | {:error, term()}
  def start_link(_opts) do
    Agent.start_link(&current_identity/0, name: __MODULE__)
  end

  @doc "The identity recorded when this BEAM started."
  @spec baseline() :: String.t()
  def baseline, do: Agent.get(__MODULE__, & &1)

  @doc """
  True when this process is running inside a fork of the VM it started
  in (the live identity no longer matches the baseline).
  """
  @spec fork?() :: boolean()
  def fork? do
    current_identity() != baseline()
  end

  @doc """
  Halt the whole BEAM if this is a fork. Called after every snapshot
  return on the spawn path; the clone stops here, the original
  continues.
  """
  @spec halt_if_fork!() :: :ok
  def halt_if_fork! do
    if fork?() do
      :init.stop()
      # Give init the time to bring the system down before the caller
      # can continue a spawn flow inside the clone.
      Process.sleep(:infinity)
    end

    :ok
  end

  @spec current_identity() :: String.t()
  defp current_identity do
    {cmd, args} =
      Application.get_env(:loom, :identity_cmd, {"sh", ["-c", default_probe()]})

    case System.cmd(cmd, args, stderr_to_stdout: true) do
      {out, 0} -> String.trim(out)
      {_out, _status} -> "unknown"
    end
  rescue
    ErlangError -> "unknown"
  end

  # Global-scope addresses sorted, so ordering churn cannot fake a fork.
  @spec default_probe() :: String.t()
  defp default_probe do
    "ip -o addr show scope global 2>/dev/null | awk '{print $4}' | sort"
  end
end
