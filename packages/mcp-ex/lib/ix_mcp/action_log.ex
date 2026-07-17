defmodule IxMcp.ActionLog do
  @moduledoc """
  Append-only SQLite record of every MCP action (#3512), normalized (#3532):
  a `sessions` row per server instance (created lazily on first use, so a
  connection that never acts leaves no row), a `topics` row per `topic_set`
  call (a timeline -- repeating a name makes a new row), and an `actions` row
  per `tools/call` referencing both. This module owns all SQLite access; the
  current session/topic ids live in `IxMcp.Session`. Pure logging for future
  reference; nothing on the hot path reads it.

  The database path resolves as app env `:actions_db` (tests pin
  `":memory:"`), then `$IX_MCP_ACTIONS_DB`, then
  `$XDG_STATE_HOME/ix-mcp-ex/actions.db` (state home defaulting to
  `~/.local/state`). A pre-#3532 database (an `actions` table without a
  `session_id` column) is migrated losslessly, in one transaction, when the
  log opens. Writes are synchronous calls on purpose: the BEAM halts as soon
  as stdin closes, so a fire-and-forget cast loses the tail of a short-lived
  session (observed live), while a call makes the row durable before the
  tool response ships; one SQLite insert is negligible against MCP wire
  overhead. A failed open or write crashes this process loudly and the
  supervisor reopens the log.
  """

  use GenServer

  alias Exqlite.Sqlite3

  # The schema is a published contract (#3532): the action-log UI is built
  # against these exact tables, so changes here must be coordinated.
  @create_sessions """
  CREATE TABLE sessions (id INTEGER PRIMARY KEY, name TEXT, started_at TEXT NOT NULL)
  """

  @create_topics """
  CREATE TABLE topics (id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id), name TEXT NOT NULL, started_at TEXT NOT NULL)
  """

  @create_actions """
  CREATE TABLE actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id), topic_id INTEGER REFERENCES topics(id), tool TEXT NOT NULL, intent TEXT, arguments TEXT NOT NULL, is_error INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL)
  """

  # The v1 shape kept session/topic as TEXT per action row. The backfill
  # makes one session per distinct v1 session string (NULLs collapse into
  # one unnamed session, hence the NULL-safe `IS` joins), one topic per
  # distinct (session, topic) pair -- v1 rows carry no topic boundaries, so
  # per-pair is the finest lossless grain -- and earliest-seen timestamps
  # stand in for the started_at v1 never stored.
  @migrate_v1 [
    "ALTER TABLE actions RENAME TO actions_v1",
    @create_sessions,
    @create_topics,
    @create_actions,
    """
    INSERT INTO sessions (name, started_at)
    SELECT session, MIN(at) FROM actions_v1 GROUP BY session
    """,
    """
    INSERT INTO topics (session_id, name, started_at)
    SELECT s.id, a.topic, MIN(a.at)
    FROM actions_v1 a JOIN sessions s ON s.name IS a.session
    WHERE a.topic IS NOT NULL
    GROUP BY s.id, a.topic
    """,
    """
    INSERT INTO actions (id, at, session_id, topic_id, tool, intent, arguments, is_error, elapsed_ms)
    SELECT a.id, a.at, s.id, t.id, a.tool, a.intent, a.arguments, a.is_error, a.elapsed_ms
    FROM actions_v1 a
    JOIN sessions s ON s.name IS a.session
    LEFT JOIN topics t ON t.session_id = s.id AND t.name IS a.topic
    """,
    "DROP TABLE actions_v1"
  ]

  @insert """
  INSERT INTO actions (at, session_id, topic_id, tool, intent, arguments, is_error, elapsed_ms)
  VALUES (?, ?, ?, ?, ?, ?, ?, ?)
  """

  @select_recent """
  SELECT a.at, s.name, t.name, a.tool, a.intent, a.arguments, a.is_error, a.elapsed_ms
  FROM actions a
  JOIN sessions s ON s.id = a.session_id
  LEFT JOIN topics t ON t.id = a.topic_id
  ORDER BY a.id DESC LIMIT ?
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

  @doc "Insert a sessions row (name may be nil); returns its id."
  @spec create_session(String.t() | nil, GenServer.server()) :: integer()
  def create_session(name, server \\ __MODULE__) do
    GenServer.call(server, {:create_session, name, now()})
  end

  @doc "Set an existing session row's name."
  @spec rename_session(integer(), String.t(), GenServer.server()) :: :ok
  def rename_session(id, name, server \\ __MODULE__) do
    GenServer.call(server, {:rename_session, id, name})
  end

  @doc "Insert a topics row under `session_id`; returns its id."
  @spec create_topic(integer(), String.t(), GenServer.server()) :: integer()
  def create_topic(session_id, name, server \\ __MODULE__) do
    GenServer.call(server, {:create_topic, session_id, name, now()})
  end

  @doc "Record one action; returns once the row is written."
  @spec record(map(), GenServer.server()) :: :ok
  def record(action, server \\ __MODULE__) do
    GenServer.call(server, {:record, Map.put(action, :at, now())})
  end

  @doc "Latest `n` recorded actions, newest first, with session/topic names joined in."
  @spec recent(pos_integer(), GenServer.server()) :: [entry()]
  def recent(n \\ 20, server \\ __MODULE__) do
    GenServer.call(server, {:recent, n})
  end

  @doc "All sessions rows, oldest first."
  @spec sessions(GenServer.server()) :: [
          %{id: integer(), name: String.t() | nil, started_at: String.t()}
        ]
  def sessions(server \\ __MODULE__) do
    GenServer.call(server, :sessions)
  end

  @doc "All topics rows, oldest first."
  @spec topics(GenServer.server()) ::
          [%{id: integer(), session_id: integer(), name: String.t(), started_at: String.t()}]
  def topics(server \\ __MODULE__) do
    GenServer.call(server, :topics)
  end

  @impl true
  def init(opts) do
    path = Keyword.get(opts, :path) || configured_path()

    if path != ":memory:", do: File.mkdir_p!(Path.dirname(path))

    {:ok, conn} = Sqlite3.open(path)

    case shape(conn) do
      :v2 -> :ok
      :empty -> execute_all(conn, [@create_sessions, @create_topics, @create_actions])
      :v1 -> execute_all(conn, ["BEGIN IMMEDIATE"] ++ @migrate_v1 ++ ["COMMIT"])
    end

    {:ok, insert} = Sqlite3.prepare(conn, @insert)
    {:ok, %{conn: conn, insert: insert}}
  end

  @impl true
  def handle_call({:create_session, name, at}, _from, %{conn: conn} = state) do
    run(conn, "INSERT INTO sessions (name, started_at) VALUES (?, ?)", [name, at])
    {:ok, id} = Sqlite3.last_insert_rowid(conn)
    {:reply, id, state}
  end

  def handle_call({:rename_session, id, name}, _from, %{conn: conn} = state) do
    run(conn, "UPDATE sessions SET name = ? WHERE id = ?", [name, id])
    {:reply, :ok, state}
  end

  def handle_call({:create_topic, session_id, name, at}, _from, %{conn: conn} = state) do
    run(conn, "INSERT INTO topics (session_id, name, started_at) VALUES (?, ?, ?)", [
      session_id,
      name,
      at
    ])

    {:ok, id} = Sqlite3.last_insert_rowid(conn)
    {:reply, id, state}
  end

  def handle_call({:record, action}, _from, %{conn: conn, insert: insert} = state) do
    :ok =
      Sqlite3.bind(insert, [
        action.at,
        action.session_id,
        action.topic_id,
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
    rows = fetch(conn, @select_recent, [n])
    {:reply, Enum.map(rows, &row_to_entry/1), state}
  end

  def handle_call(:sessions, _from, %{conn: conn} = state) do
    rows =
      for [id, name, started_at] <-
            fetch(conn, "SELECT id, name, started_at FROM sessions ORDER BY id", []) do
        %{id: id, name: name, started_at: started_at}
      end

    {:reply, rows, state}
  end

  def handle_call(:topics, _from, %{conn: conn} = state) do
    rows =
      for [id, session_id, name, started_at] <-
            fetch(conn, "SELECT id, session_id, name, started_at FROM topics ORDER BY id", []) do
        %{id: id, session_id: session_id, name: name, started_at: started_at}
      end

    {:reply, rows, state}
  end

  defp shape(conn) do
    case table_columns(conn, "actions") do
      [] -> :empty
      columns -> if "session_id" in columns, do: :v2, else: :v1
    end
  end

  defp table_columns(conn, table) do
    for [_cid, name | _rest] <- fetch(conn, "PRAGMA table_info(#{table})", []), do: name
  end

  defp execute_all(conn, statements) do
    Enum.each(statements, fn statement -> :ok = Sqlite3.execute(conn, statement) end)
  end

  defp run(conn, sql, params) do
    {:ok, statement} = Sqlite3.prepare(conn, sql)
    :ok = Sqlite3.bind(statement, params)
    :done = Sqlite3.step(conn, statement)
    :ok = Sqlite3.release(conn, statement)
  end

  defp fetch(conn, sql, params) do
    {:ok, statement} = Sqlite3.prepare(conn, sql)
    :ok = Sqlite3.bind(statement, params)
    {:ok, rows} = Sqlite3.fetch_all(conn, statement)
    :ok = Sqlite3.release(conn, statement)
    rows
  end

  defp configured_path do
    Application.get_env(:ix_mcp, :actions_db) ||
      System.get_env("IX_MCP_ACTIONS_DB") ||
      Path.join([state_home(), "ix-mcp-ex", "actions.db"])
  end

  defp state_home do
    System.get_env("XDG_STATE_HOME") || Path.join(System.user_home!(), ".local/state")
  end

  defp now, do: DateTime.utc_now() |> DateTime.to_iso8601()

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
