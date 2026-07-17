defmodule IxMcp.ActionLog do
  @moduledoc """
  Append-only SQLite record of every MCP action (#3512): one row per
  `tools/call` with the session/topic active at the time, the tool name and
  arguments, whether it errored, and how long it took. Pure logging for
  future reference; nothing on the hot path reads it.

  The database path resolves as app env `:actions_db` (tests pin
  `":memory:"`), then `$IX_MCP_ACTIONS_DB`, then
  `$XDG_STATE_HOME/ix-mcp-ex/actions.db` (state home defaulting to
  `~/.local/state`). Writes are synchronous calls on purpose: the BEAM halts
  as soon as stdin closes, so a fire-and-forget cast loses the tail of a
  short-lived session (observed live), while a call makes the row durable
  before the tool response ships; one SQLite insert is negligible against
  MCP wire overhead. A failed open or write crashes this process loudly and
  the supervisor reopens the log.
  """

  use GenServer

  alias Exqlite.Sqlite3

  @schema """
  CREATE TABLE IF NOT EXISTS actions (
    id INTEGER PRIMARY KEY,
    at TEXT NOT NULL,
    session TEXT,
    topic TEXT,
    tool TEXT NOT NULL,
    intent TEXT,
    arguments TEXT NOT NULL,
    is_error INTEGER NOT NULL,
    elapsed_ms INTEGER NOT NULL
  )
  """

  @insert """
  INSERT INTO actions (at, session, topic, tool, intent, arguments, is_error, elapsed_ms)
  VALUES (?, ?, ?, ?, ?, ?, ?, ?)
  """

  @select_recent """
  SELECT at, session, topic, tool, intent, arguments, is_error, elapsed_ms
  FROM actions ORDER BY id DESC LIMIT ?
  """

  @type entry :: %{
          at: String.t(),
          session: String.t() | nil,
          topic: String.t() | nil,
          tool: String.t(),
          intent: String.t() | nil,
          arguments: String.t(),
          is_error: boolean(),
          elapsed_ms: non_neg_integer()
        }

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: Keyword.get(opts, :name, __MODULE__))
  end

  @doc "Record one action; returns once the row is written."
  @spec record(map(), GenServer.server()) :: :ok
  def record(action, server \\ __MODULE__) do
    at = DateTime.utc_now() |> DateTime.to_iso8601()
    GenServer.call(server, {:record, Map.put(action, :at, at)})
  end

  @doc "Latest `n` recorded actions, newest first."
  @spec recent(pos_integer(), GenServer.server()) :: [entry()]
  def recent(n \\ 20, server \\ __MODULE__) do
    GenServer.call(server, {:recent, n})
  end

  @impl true
  def init(opts) do
    path = Keyword.get(opts, :path) || configured_path()

    if path != ":memory:", do: File.mkdir_p!(Path.dirname(path))

    {:ok, conn} = Sqlite3.open(path)
    :ok = Sqlite3.execute(conn, @schema)
    {:ok, insert} = Sqlite3.prepare(conn, @insert)
    {:ok, %{conn: conn, insert: insert}}
  end

  @impl true
  def handle_call({:record, action}, _from, %{conn: conn, insert: insert} = state) do
    :ok =
      Sqlite3.bind(insert, [
        action.at,
        action.session,
        action.topic,
        action.tool,
        action.intent,
        action.arguments,
        bool_to_int(action.is_error),
        action.elapsed_ms
      ])

    :done = Sqlite3.step(conn, insert)
    {:reply, :ok, state}
  end

  def handle_call({:recent, n}, _from, %{conn: conn} = state) do
    {:ok, select} = Sqlite3.prepare(conn, @select_recent)
    :ok = Sqlite3.bind(select, [n])
    {:ok, rows} = Sqlite3.fetch_all(conn, select)
    :ok = Sqlite3.release(conn, select)
    {:reply, Enum.map(rows, &row_to_entry/1), state}
  end

  defp configured_path do
    Application.get_env(:ix_mcp, :actions_db) ||
      System.get_env("IX_MCP_ACTIONS_DB") ||
      Path.join([state_home(), "ix-mcp-ex", "actions.db"])
  end

  defp state_home do
    System.get_env("XDG_STATE_HOME") || Path.join(System.user_home!(), ".local/state")
  end

  defp bool_to_int(true), do: 1
  defp bool_to_int(false), do: 0

  defp row_to_entry([at, session, topic, tool, intent, arguments, is_error, elapsed_ms]) do
    %{
      at: at,
      session: session,
      topic: topic,
      tool: tool,
      intent: intent,
      arguments: arguments,
      is_error: is_error == 1,
      elapsed_ms: elapsed_ms
    }
  end
end
