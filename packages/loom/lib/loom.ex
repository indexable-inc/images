defmodule Loom do
  @moduledoc """
  Fork-the-workstation subagents on ix VMs: the facade.

      {:ok, id} = Loom.spawn("audit packages/foo and fix the flaky test")
      # ... {:loom, ^id, {:spawned, vm}} then {:loom, ^id, {:final, text}}
      Loom.send_text(id, "now update the changelog")
      Loom.delete(id)

  Each spawn snapshots the parent VM (the control VM this kernel runs
  in, or `LOOM_PARENT_VM` when driven from a workstation), restores the
  snapshot into a fresh VM, and runs a headless claude child inside it
  through `ix shell`. See `Loom.Agent` for the state machine and the
  README for the whole design.
  """

  alias Loom.Agent

  @typedoc "Agent id, as returned by `spawn/2`."
  @type id :: String.t()

  @doc """
  Spawn a subagent working on `brief` in a fork of the parent VM.

  Options:

    * `:owner` - pid receiving `{:loom, id, event}` messages (default:
      the caller);
    * `:parent_vm` - the VM to fork (default: the `:loom` app env, then
      `LOOM_PARENT_VM`).
  """
  @spec spawn(String.t(), keyword()) :: {:ok, id()} | {:error, :no_parent_vm | term()}
  def spawn(brief, opts \\ []) when is_binary(brief) do
    case resolve_parent(opts) do
      {:ok, parent_vm} ->
        id = new_id()
        opts = [owner: Keyword.get(opts, :owner, self()), parent_vm: parent_vm]

        case DynamicSupervisor.start_child(Loom.AgentSupervisor, {Agent, {id, brief, opts}}) do
          {:ok, _pid} -> {:ok, id}
          {:error, reason} -> {:error, reason}
        end

      {:error, reason} ->
        {:error, reason}
    end
  end

  @doc "Phase, VM name, session and result for one agent."
  @spec status(id()) :: {:ok, map()} | {:error, :not_found}
  defdelegate status(id), to: Agent

  @doc "Deliver a follow-up to an agent; wakes it when idle."
  @spec send_text(id(), String.t()) :: :ok | {:error, term()}
  defdelegate send_text(id, text), to: Agent

  @doc "Delete the agent's VM and terminate it."
  @spec delete(id()) :: :ok | {:error, :not_found}
  defdelegate delete(id), to: Agent

  @doc "All live agent ids."
  @spec list() :: [id()]
  def list do
    Registry.select(Loom.Registry, [{{:"$1", :_, :_}, [], [:"$1"]}])
  end

  @spec resolve_parent(keyword()) :: {:ok, String.t()} | {:error, :no_parent_vm}
  defp resolve_parent(opts) do
    configured =
      Keyword.get(opts, :parent_vm) ||
        Application.get_env(:loom, :parent_vm) ||
        System.get_env("LOOM_PARENT_VM")

    case configured do
      nil -> {:error, :no_parent_vm}
      vm -> {:ok, vm}
    end
  end

  # Short, url/hostname-safe, collision-unlikely at loom scale; the VM
  # name derives from it ("loom-<id>"), so keep it lowercase hex.
  @spec new_id() :: id()
  defp new_id do
    Base.encode16(:crypto.strong_rand_bytes(3), case: :lower)
  end
end
