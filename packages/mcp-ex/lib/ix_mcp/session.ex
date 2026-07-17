defmodule IxMcp.Session do
  @moduledoc """
  Dashboard-facing metadata for this server instance: a human task label and a
  current topic. Jobs record the session/topic active when they started, so a
  feed of runs groups the same way the Python dashboard grouped them.
  """

  use Agent

  @type t :: %{name: String.t() | nil, topic: String.t() | nil}

  @spec start_link(term()) :: Agent.on_start()
  def start_link(_opts) do
    Agent.start_link(fn -> %{name: nil, topic: nil} end, name: __MODULE__)
  end

  @spec set_name(String.t()) :: :ok
  def set_name(name) when is_binary(name) do
    Agent.update(__MODULE__, &%{&1 | name: name})
  end

  @spec set_topic(String.t()) :: :ok
  def set_topic(topic) when is_binary(topic) do
    Agent.update(__MODULE__, &%{&1 | topic: topic})
  end

  @spec get() :: t()
  def get do
    Agent.get(__MODULE__, & &1)
  end
end
