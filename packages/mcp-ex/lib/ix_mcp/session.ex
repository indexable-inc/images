defmodule IxMcp.Session do
  @moduledoc """
  This server instance's identity in the action log, plus the dashboard-facing
  labels: one instance = one `sessions` row, created lazily on first use so a
  connection that never acts leaves no row (#3532). The current session and
  topic ids live here; all SQLite access lives in `IxMcp.ActionLog`. Jobs
  record the session/topic names active when they started, so a feed of runs
  groups the same way the Python dashboard grouped them.
  """

  use Agent

  alias IxMcp.ActionLog

  @type t :: %{
          name: String.t() | nil,
          topic: String.t() | nil,
          session_id: integer() | nil,
          topic_id: integer() | nil
        }

  @spec start_link(term()) :: Agent.on_start()
  def start_link(_opts) do
    Agent.start_link(
      fn -> %{name: nil, topic: nil, session_id: nil, topic_id: nil} end,
      name: __MODULE__
    )
  end

  @doc "Name this session's row, creating it when naming precedes any action."
  @spec set_name(String.t()) :: :ok
  def set_name(name) when is_binary(name) do
    Agent.update(__MODULE__, fn
      %{session_id: nil} = state ->
        %{state | name: name, session_id: create_row(name)}

      state ->
        :ok = ActionLog.rename_session(state.session_id, name)
        %{state | name: name}
    end)
  end

  @doc """
  Start a new topic: every call inserts a fresh topics row (repeating a name
  makes a new row -- the log is a timeline, not a dictionary) and makes it
  current.
  """
  @spec set_topic(String.t()) :: :ok
  def set_topic(topic) when is_binary(topic) do
    Agent.update(__MODULE__, fn state ->
      state = ensure_session(state)
      %{state | topic: topic, topic_id: ActionLog.create_topic(state.session_id, topic)}
    end)
  end

  @doc "Current session/topic ids for recording an action; creates the session row lazily."
  @spec ids() :: %{session_id: integer(), topic_id: integer() | nil}
  def ids do
    Agent.get_and_update(__MODULE__, fn state ->
      state = ensure_session(state)
      {%{session_id: state.session_id, topic_id: state.topic_id}, state}
    end)
  end

  @doc "The display labels (what jobs stamp on their history entries)."
  @spec get() :: %{name: String.t() | nil, topic: String.t() | nil}
  def get do
    Agent.get(__MODULE__, &Map.take(&1, [:name, :topic]))
  end

  defp ensure_session(%{session_id: nil} = state) do
    %{state | session_id: create_row(state.name)}
  end

  defp ensure_session(state), do: state

  # The spawn tag rides the environment because the spawner is outside the
  # BEAM: a wrapper (claude-html) sets IX_MCP_SPAWN_TAG on the `claude` it
  # launches, the kernel inherits it, and the row it stamps here is how the
  # wrapper finds this session among every other one sharing the database
  # (ENG-12004).
  defp create_row(name) do
    ActionLog.create_session(name, ActionLog, spawn_tag: System.get_env("IX_MCP_SPAWN_TAG"))
  end
end
