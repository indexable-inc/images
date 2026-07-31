defmodule IxMcp.Agents.Control do
  @moduledoc """
  Where a running child's steering handles live: the runner process that owns
  its CLI port, the OS pid that port spawned, and whether that CLI has a stdin
  channel to steer through at all.

  A `Registry` rather than fields in `IxMcp.Agents.Events`, for two properties
  the ledger cannot give. An entry disappears when the runner dies, so nothing
  can write into a closed port. And `interrupt/1` needs the runner's PID rather
  than the port: `Port.command/2` raises `badarg` from any process but the
  port's owner, so an interrupt has to travel through the runner's own receive
  loop.

  Keys are duplicate, not unique, for a reason that shows up on the second
  phase of a child's life. A woken child starts a new runner task while the
  previous phase's entry may not be swept yet (Registry cleans up on a monitor
  message, promptly but asynchronously), and a unique key would make that a
  registration error to retry around. Duplicate keys plus an aliveness filter
  in `lookup/1` make the race unrepresentable instead.
  """

  @type entry :: %{
          runner: pid(),
          os_pid: pos_integer() | nil,
          stdin: :stream | :closed,
          backend: atom()
        }

  @spec child_spec(term()) :: Supervisor.child_spec()
  def child_spec(_opts) do
    Registry.child_spec(keys: :duplicate, name: __MODULE__)
  end

  @doc "Called by the runner itself, so the entry's lifetime is the phase's."
  @spec register(String.t(), entry()) :: :ok
  def register(agent_id, %{runner: runner} = entry) when runner == self() do
    {:ok, _pid} = Registry.register(__MODULE__, agent_id, entry)
    :ok
  end

  @doc "The live entry for one agent, newest first when a phase is turning over."
  @spec lookup(String.t()) :: {:ok, entry()} | :error
  def lookup(agent_id) do
    case live_entries(agent_id) do
      [entry | _older] -> {:ok, entry}
      [] -> :error
    end
  end

  @doc "Every agent with a live runner right now."
  @spec all() :: %{String.t() => entry()}
  def all do
    __MODULE__
    |> Registry.select([{{:"$1", :"$2", :"$3"}, [], [{{:"$1", :"$3"}}]}])
    |> Enum.filter(fn {_id, entry} -> Process.alive?(entry.runner) end)
    |> Map.new()
  end

  defp live_entries(agent_id) do
    __MODULE__
    |> Registry.lookup(agent_id)
    |> Enum.filter(fn {pid, _entry} -> Process.alive?(pid) end)
    |> Enum.map(fn {_pid, entry} -> entry end)
  end
end
