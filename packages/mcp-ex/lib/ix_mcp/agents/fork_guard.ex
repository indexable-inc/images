defmodule IxMcp.Agents.ForkGuard do
  @moduledoc """
  Records the control VM's network identity before any Loom snapshot.

  A restored snapshot resumes this BEAM in the child VM. The address change is
  the signal that this is the clone, which must stop before it continues the
  parent's spawn path and recursively creates more VMs.
  """

  use Agent

  @spec start_link(keyword()) :: :ignore | {:ok, pid()} | {:error, term()}
  def start_link(opts) do
    if Keyword.fetch!(opts, :enabled) do
      Agent.start_link(&current_identity/0, name: __MODULE__)
    else
      :ignore
    end
  end

  @doc "Stop this BEAM when a snapshot restored it under a new VM identity."
  @spec halt_if_fork!() :: :ok
  def halt_if_fork! do
    if current_identity() != Agent.get(__MODULE__, & &1) do
      :init.stop()
      Process.sleep(:infinity)
    end

    :ok
  end

  defp current_identity do
    probe = "ip -o addr show scope global 2>/dev/null | awk '{print $4}' | sort"

    case System.cmd("sh", ["-c", probe], stderr_to_stdout: true) do
      {out, 0} -> String.trim(out)
      {_out, _status} -> "unknown"
    end
  rescue
    ErlangError -> "unknown"
  end
end
