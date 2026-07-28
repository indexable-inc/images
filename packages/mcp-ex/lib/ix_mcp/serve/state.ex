defmodule IxMcp.Serve.State do
  @moduledoc """
  Serve bookkeeping: app dir -> url, job ids, last gate outcome. An Agent so
  the gate loop's writes and `Serve.status/1` reads serialize; the jobs may
  die, the record stays until `Serve.stop/1` clears it.
  """

  use Agent

  @spec start_link(term()) :: Agent.on_start()
  def start_link(_opts) do
    Agent.start_link(fn -> %{} end, name: __MODULE__)
  end

  @spec put(String.t(), map()) :: :ok
  def put(dir, entry) when is_binary(dir) and is_map(entry) do
    Agent.update(__MODULE__, &Map.put(&1, dir, entry))
  end

  @doc "Merge `fields` into the dir's entry; a missing entry starts from `fields`."
  @spec merge(String.t(), map()) :: :ok
  def merge(dir, fields) when is_binary(dir) and is_map(fields) do
    Agent.update(__MODULE__, fn state ->
      Map.update(state, dir, fields, &Map.merge(&1, fields))
    end)
  end

  @spec get(String.t()) :: map() | nil
  def get(dir) when is_binary(dir) do
    Agent.get(__MODULE__, &Map.get(&1, dir))
  end

  @spec delete(String.t()) :: :ok
  def delete(dir) when is_binary(dir) do
    Agent.update(__MODULE__, &Map.delete(&1, dir))
  end
end
