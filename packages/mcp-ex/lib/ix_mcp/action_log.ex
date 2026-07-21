defmodule IxMcp.ActionLog do
  @moduledoc """
  Append-only SQLite record of every MCP action (#3512), normalized (#3532):
  a `sessions` row per server instance (created lazily on first use, so a
  connection that never acts leaves no row), a `topics` row per `topic_set`
  call (a timeline -- repeating a name makes a new row), and an `actions` row
  per `tools/call` referencing both. This module owns all SQLite access; the
  current session/topic ids live in `IxMcp.Session`. Pure logging for future
  reference; nothing on the hot path reads it.

  Action rows are live (#3536): inserted with `status = 'running'` when the
  call arrives (`start_action/2`) and finalized to `done`/`failed`/
  `cancelled` when its work completes (`finish_action/5`) -- for `exec` that
  is when the eval finishes, which for a backgrounded job is after the wire
  response shipped. While an exec runs, its job samples the eval process's
  `current_stacktrace` into `stack`/`stack_at` -- plus, when the stack has a
  frame the cell itself owns, the 1-based cell source line into `line`
  (`update_stack/4`, #3546) -- so a reader can show the line a hung cell
  sits on and highlight it in the rendered cell source. There is deliberately no
  startup sweep of leftover `running` rows: several server instances share
  one database, so a sweep would clobber a live sibling's rows. A row whose
  kernel died stays `running` with a frozen `stack_at`; readers judge
  liveness by that freshness (samples land every second).

  The database path resolves as app env `:actions_db` (tests pin
  `":memory:"`), then `$IX_MCP_ACTIONS_DB`, then
  `$XDG_STATE_HOME/ix-mcp-ex/actions.db` (state home defaulting to
  `~/.local/state`). The schema carries its version in `PRAGMA
  user_version` (#3539): an older database migrates forward through the
  ordered steps (losslessly, one transaction per step) when the log opens,
  while a database stamped by a newer server is refused -- loudly, but
  without failing startup: logging disables for this instance, because a
  tool server must not die over its own log. Writes are synchronous calls on purpose: the BEAM halts as soon
  as stdin closes, so a fire-and-forget cast loses the tail of a short-lived
  session (observed live), while a call makes the row durable before the
  tool response ships; one SQLite insert is negligible against MCP wire
  overhead. A failed open or write crashes this process loudly and the
  supervisor reopens the log.
  """

  use GenServer

  alias Exqlite.Sqlite3
  alias IxMcp.Jobs.Job

  require Logger

  # The schema is a published contract (#3532): the action-log UI is built
  # against these exact tables, so changes here must be coordinated.
  @create_sessions """
  CREATE TABLE sessions (id INTEGER PRIMARY KEY, name TEXT, started_at TEXT NOT NULL)
  """

  @create_topics """
  CREATE TABLE topics (id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id), name TEXT NOT NULL, started_at TEXT NOT NULL)
  """

  @create_actions """
  CREATE TABLE actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id), topic_id INTEGER REFERENCES topics(id), tool TEXT NOT NULL, intent TEXT, arguments TEXT NOT NULL, is_error INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'done', stack TEXT, stack_at TEXT, line INTEGER)
  """

  # index#3839: a durable ledger for background jobs. Every job status
  # transition (running -> done|failed|cancelled|killed) is one atomic write
  # here, in the same transaction that drives the job's `actions` row terminal
  # and inserts an outbox row -- so no job death can leave the log disagreeing
  # with itself or notify silently. Output streams into `job_output` (batched
  # by the job process) so `Jobs.tail/output/result` read from disk after the
  # job process is gone; `outbox` holds terminal-transition notifications for
  # replay to a transport that (re)connects.
  @create_jobs """
  CREATE TABLE jobs (id TEXT PRIMARY KEY, session_id INTEGER REFERENCES sessions(id), action_id INTEGER REFERENCES actions(id), intent TEXT, session_name TEXT, topic_name TEXT, code TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'running', watch INTEGER NOT NULL DEFAULT 0, result TEXT, output_bytes INTEGER NOT NULL DEFAULT 0, output_dropped INTEGER NOT NULL DEFAULT 0, started_at TEXT NOT NULL, finished_at TEXT, elapsed_ms INTEGER)
  """

  @create_job_output """
  CREATE TABLE job_output (job_id TEXT NOT NULL REFERENCES jobs(id), seq INTEGER NOT NULL, chunk TEXT NOT NULL, PRIMARY KEY (job_id, seq))
  """

  @create_outbox """
  CREATE TABLE outbox (id INTEGER PRIMARY KEY, job_id TEXT, intent TEXT, status TEXT NOT NULL, elapsed_ms INTEGER, result TEXT, created_at TEXT NOT NULL, acked INTEGER NOT NULL DEFAULT 0)
  """

  # index#3539: the schema version is stamped into SQLite's `PRAGMA
  # user_version` header field instead of being re-derived by column
  # sniffing on every open. Sniffing can only classify shapes this binary
  # already knows: when the on-disk schema is NEWER than the reader (the
  # 2026-07-17 incident -- a #3512-era binary starting against the file the
  # #3532 normalization had already rewritten), every sniff answer is wrong
  # and the mismatch surfaces as a match-crash deep in a prepare. A stamped
  # version makes older shapes migratable in order, the current shape a
  # no-op, and a future shape explicitly detectable. The ladder: 1 = the
  # flat #3512 log, 2 = the #3532 normalization, 3 = the #3536 live rows,
  # 4 = the #3546 live cell line, 5 = the #3839 durable job ledger.
  @user_version 5

  # Frozen historical DDL for the 1 -> 2 step: the actions shape exactly as
  # #3532 shipped it, before the live-row columns. A migration must never
  # borrow the current @create_actions, or editing today's schema would
  # silently rewrite the ladder's history.
  @create_actions_v2 """
  CREATE TABLE actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id), topic_id INTEGER REFERENCES topics(id), tool TEXT NOT NULL, intent TEXT, arguments TEXT NOT NULL, is_error INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL)
  """

  # The v1 shape kept session/topic as TEXT per action row. The backfill
  # makes one session per distinct v1 session string (NULLs collapse into
  # one unnamed session, hence the NULL-safe `IS` joins), one topic per
  # distinct (session, topic) pair -- v1 rows carry no topic boundaries, so
  # per-pair is the finest lossless grain -- and earliest-seen timestamps
  # stand in for the started_at v1 never stored.
  @migrate_v1_to_v2 [
    "ALTER TABLE actions RENAME TO actions_v1",
    @create_sessions,
    @create_topics,
    @create_actions_v2,
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

  # A v2 database predates the live-row columns (#3536); DEFAULT 'done' is
  # exactly right for its rows, which were all written after the fact.
  @migrate_v2_to_v3 [
    "ALTER TABLE actions ADD COLUMN status TEXT NOT NULL DEFAULT 'done'",
    "ALTER TABLE actions ADD COLUMN stack TEXT",
    "ALTER TABLE actions ADD COLUMN stack_at TEXT"
  ]

  # A v3 database predates the sampled cell line (#3546); NULL is exactly
  # right for its rows, whose samples never carried one.
  @migrate_v3_to_v4 [
    "ALTER TABLE actions ADD COLUMN line INTEGER"
  ]

  # A v4 database predates the durable job ledger (#3839): the three new
  # tables are simply created empty, so pre-#3839 rows are untouched.
  @migrate_v4_to_v5 [@create_jobs, @create_job_output, @create_outbox]

  # Ordered migrations keyed by the user_version each upgrades FROM. Every
  # step runs in one immediate transaction that also stamps the version it
  # produces, so an interrupted migration leaves the previous consistent,
  # correctly-stamped version on disk.
  @migrations [
    {1, @migrate_v1_to_v2},
    {2, @migrate_v2_to_v3},
    {3, @migrate_v3_to_v4},
    {4, @migrate_v4_to_v5}
  ]

  @insert """
  INSERT INTO actions (at, session_id, topic_id, tool, intent, arguments, is_error, elapsed_ms, status)
  VALUES (?, ?, ?, ?, ?, ?, 0, 0, 'running')
  """

  # Both updates guard on status = 'running': a finalize is idempotent and a
  # stack sample racing the finish can never resurrect a finished row.
  @finish """
  UPDATE actions SET status = ?, is_error = ?, elapsed_ms = ?, stack = NULL, stack_at = NULL, line = NULL
  WHERE id = ? AND status = 'running'
  """

  @update_stack """
  UPDATE actions SET stack = ?, stack_at = ?, line = ? WHERE id = ? AND status = 'running'
  """

  @select_recent """
  SELECT a.at, s.name, t.name, a.tool, a.intent, a.arguments, a.is_error, a.elapsed_ms, a.status, a.stack, a.line
  FROM actions a
  JOIN sessions s ON s.id = a.session_id
  LEFT JOIN topics t ON t.id = a.topic_id
  ORDER BY a.id DESC LIMIT ?
  """

  @select_job """
  SELECT id, intent, session_name, topic_name, code, status, watch, result, output_bytes, output_dropped, started_at, finished_at, elapsed_ms
  FROM jobs
  """

  @type entry :: %{
          at: String.t(),
          session: String.t() | nil,
          topic: String.t() | nil,
          tool: String.t(),
          intent: String.t() | nil,
          arguments: String.t(),
          is_error: boolean(),
          elapsed_ms: non_neg_integer(),
          status: String.t(),
          stack: String.t() | nil,
          line: pos_integer() | nil
        }

  @typedoc "Fields for inserting a fresh `jobs` row (#3839)."
  @type job_start :: %{
          id: String.t(),
          session_id: integer() | nil,
          action_id: integer() | nil,
          intent: String.t() | nil,
          session_name: String.t() | nil,
          topic_name: String.t() | nil,
          code: String.t(),
          watch: boolean(),
          started_at: String.t()
        }

  @typedoc "A recorded job row (#3839)."
  @type job :: %{
          id: String.t(),
          intent: String.t() | nil,
          session: String.t() | nil,
          topic: String.t() | nil,
          code: String.t(),
          status: Job.status(),
          watch: boolean(),
          result: String.t() | nil,
          output_bytes: non_neg_integer(),
          output_dropped: non_neg_integer(),
          started_at: String.t(),
          finished_at: String.t() | nil,
          elapsed_ms: non_neg_integer() | nil
        }

  @typedoc "A terminal-transition notification awaiting delivery (#3839)."
  @type outbox :: %{
          id: integer(),
          job_id: String.t(),
          intent: String.t() | nil,
          status: Job.status(),
          elapsed_ms: non_neg_integer() | nil,
          result: String.t() | nil
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

  @doc "Insert an action row as `running` before the call executes; returns its id."
  @spec start_action(map(), GenServer.server()) :: integer()
  def start_action(action, server \\ __MODULE__) do
    GenServer.call(server, {:start_action, Map.put(action, :at, now())})
  end

  @doc "Finalize a running action row; a no-op when it already finished."
  @spec finish_action(integer(), String.t(), boolean(), non_neg_integer(), GenServer.server()) ::
          :ok
  def finish_action(id, status, is_error, elapsed_ms, server \\ __MODULE__)
      when status in ["done", "failed", "cancelled"] do
    GenServer.call(server, {:finish_action, id, status, is_error, elapsed_ms})
  end

  @doc """
  Refresh a running action row's sampled stack (JSON frames) and the cell
  source line the eval currently sits on (nil when the sample has no frame
  the cell owns, #3546); a no-op once finished.
  """
  @spec update_stack(integer(), String.t(), pos_integer() | nil, GenServer.server()) :: :ok
  def update_stack(id, stack_json, line, server \\ __MODULE__) do
    GenServer.call(server, {:update_stack, id, stack_json, line, now()})
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

  # -- durable job ledger (#3839) --------------------------------------------

  @doc "Insert a `jobs` row as `running` when a job starts; idempotent per id."
  @spec job_started(job_start(), GenServer.server()) :: :ok
  def job_started(job, server \\ __MODULE__) do
    GenServer.call(server, {:job_started, job})
  end

  @doc """
  Append output `chunks` (`[{seq, chunk}]`) to a job, recording `dropped`
  bytes discarded by the per-job cap. Batched by the job process so the hot
  path is not one call per `put_chars`.
  """
  @spec append_job_output(
          String.t(),
          [{integer(), binary()}],
          non_neg_integer(),
          GenServer.server()
        ) ::
          :ok
  def append_job_output(id, chunks, dropped \\ 0, server \\ __MODULE__) do
    GenServer.call(server, {:append_job_output, id, chunks, dropped})
  end

  @doc """
  Commit a terminal job transition atomically: drive the `jobs` row terminal,
  drive its `actions` row terminal in the same transaction, and insert an
  outbox row for the notification. Returns `{:notify, outbox}` for the caller
  to deliver, or `:already_final` when the job already transitioned -- which
  is how the executor and the reaper race harmlessly (#3839).
  """
  @spec finish_job(String.t(), Job.status(), String.t() | nil, GenServer.server()) ::
          {:notify, outbox()} | :already_final
  def finish_job(id, status, result, server \\ __MODULE__)
      when status in [:done, :failed, :cancelled, :killed] do
    GenServer.call(server, {:finish_job, id, Atom.to_string(status), result, now()})
  end

  @doc "The recorded job row, or nil."
  @spec job(String.t(), GenServer.server()) :: job() | nil
  def job(id, server \\ __MODULE__) do
    GenServer.call(server, {:job, id})
  end

  @doc "The full recorded output of a job, from the durable table."
  @spec job_output(String.t(), GenServer.server()) :: binary()
  def job_output(id, server \\ __MODULE__) do
    GenServer.call(server, {:job_output, id})
  end

  @doc "Recent jobs for a session (nil = all), newest first."
  @spec recent_jobs(integer() | nil, pos_integer(), GenServer.server()) :: [job()]
  def recent_jobs(session_id, n \\ 20, server \\ __MODULE__) do
    GenServer.call(server, {:recent_jobs, session_id, n})
  end

  @doc "Unacked outbox rows (terminal-transition notifications), oldest first."
  @spec unacked_outbox(GenServer.server()) :: [outbox()]
  def unacked_outbox(server \\ __MODULE__) do
    GenServer.call(server, :unacked_outbox)
  end

  @doc "Mark outbox rows delivered."
  @spec ack_outbox([integer()], GenServer.server()) :: :ok
  def ack_outbox(ids, server \\ __MODULE__) do
    GenServer.call(server, {:ack_outbox, ids})
  end

  @doc """
  The resolved database path: app env `:actions_db` (tests pin `":memory:"`),
  then `$IX_MCP_ACTIONS_DB`, then `$XDG_STATE_HOME/ix-mcp-ex/actions.db`.
  Public because crash-dump routing (`IxMcp.Application`) aims
  `ERL_CRASH_DUMP` at the same directory (#3539).
  """
  @spec db_path() :: String.t()
  def db_path do
    Application.get_env(:ix_mcp, :actions_db) ||
      System.get_env("IX_MCP_ACTIONS_DB") ||
      Path.join([state_home(), "ix-mcp-ex", "actions.db"])
  end

  @impl true
  def init(opts) do
    path = Keyword.get(opts, :path) || db_path()

    if path != ":memory:", do: File.mkdir_p!(Path.dirname(path))

    {:ok, conn} = Sqlite3.open(path)

    # index#3539: on 2026-07-17 a server binary match-crashed right here
    # against an action log written under a newer schema, and the failed
    # child took the whole application down -- every tool call died over a
    # log nothing on the hot path reads. Older on-disk versions migrate
    # forward; a future version (a newer server already ran against this
    # file) refuses loudly but keeps the server up, degraded to not
    # recording, so the blast radius stays scoped to the log itself.
    case ensure_version(conn) do
      :ok ->
        {:ok, insert} = Sqlite3.prepare(conn, @insert)
        {:ok, %{conn: conn, insert: insert}}

      {:future, found} ->
        :ok = Sqlite3.close(conn)

        Logger.error(
          "action log #{path} has schema user_version #{found}, newer than the supported " <>
            "#{@user_version}: a newer ix-mcp-ex has run against this file. Upgrade this " <>
            "server or point IX_MCP_ACTIONS_DB at a fresh path; not recording actions for " <>
            "this instance (index#3539)."
        )

        {:ok, :disabled}
    end
  end

  # index#3539 degraded mode: the file belongs to a newer server, so writes
  # are dropped and reads answer empty rather than crashing every caller.
  # Ids still come back as integers because IxMcp.Session stores them only
  # to hand them straight back to this module, where they are ignored.
  @impl true
  def handle_call(request, _from, :disabled) do
    {:reply, disabled_reply(request), :disabled}
  end

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

  def handle_call({:start_action, action}, _from, %{conn: conn, insert: insert} = state) do
    :ok =
      Sqlite3.bind(insert, [
        action.at,
        action.session_id,
        action.topic_id,
        action.tool,
        action.intent,
        action.arguments
      ])

    :done = Sqlite3.step(conn, insert)
    {:ok, id} = Sqlite3.last_insert_rowid(conn)
    {:reply, id, state}
  end

  def handle_call(
        {:finish_action, id, status, is_error, elapsed_ms},
        _from,
        %{conn: conn} = state
      ) do
    run(conn, @finish, [status, bool_to_int(is_error), elapsed_ms, id])
    {:reply, :ok, state}
  end

  def handle_call({:update_stack, id, stack_json, line, at}, _from, %{conn: conn} = state) do
    run(conn, @update_stack, [stack_json, at, line, id])
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

  def handle_call({:job_started, job}, _from, %{conn: conn} = state) do
    run(
      conn,
      "INSERT OR IGNORE INTO jobs (id, session_id, action_id, intent, session_name, topic_name, code, status, watch, started_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'running', ?, ?)",
      [
        job.id,
        job.session_id,
        job.action_id,
        job.intent,
        job.session_name,
        job.topic_name,
        job.code,
        bool_to_int(job.watch),
        job.started_at
      ]
    )

    {:reply, :ok, state}
  end

  def handle_call({:append_job_output, id, chunks, dropped}, _from, %{conn: conn} = state) do
    :ok = Sqlite3.execute(conn, "BEGIN IMMEDIATE")

    added =
      Enum.reduce(chunks, 0, fn {seq, chunk}, acc ->
        run(conn, "INSERT OR IGNORE INTO job_output (job_id, seq, chunk) VALUES (?, ?, ?)", [
          id,
          seq,
          chunk
        ])

        acc + byte_size(chunk)
      end)

    run(
      conn,
      "UPDATE jobs SET output_bytes = output_bytes + ?, output_dropped = output_dropped + ? WHERE id = ?",
      [added, dropped, id]
    )

    :ok = Sqlite3.execute(conn, "COMMIT")
    {:reply, :ok, state}
  end

  # The one atomic terminal transition (#3839): the SELECT is the idempotency
  # guard (only a still-running job transitions), and the jobs row, its
  # actions row, and the outbox insert all commit together, so a reader can
  # never catch the two logs disagreeing and no terminal transition escapes
  # the outbox. The single-writer GenServer serializes this against every
  # other write, so the guard needs no locking of its own.
  def handle_call({:finish_job, id, status, result, at}, _from, %{conn: conn} = state) do
    reply =
      case fetch(
             conn,
             "SELECT action_id, intent, started_at FROM jobs WHERE id = ? AND status = 'running'",
             [id]
           ) do
        [] ->
          :already_final

        [[action_id, intent, started_at]] ->
          elapsed = elapsed_ms(started_at, at)
          is_error = if status in ["failed", "killed"], do: 1, else: 0
          :ok = Sqlite3.execute(conn, "BEGIN IMMEDIATE")

          run(
            conn,
            "UPDATE jobs SET status = ?, result = ?, finished_at = ?, elapsed_ms = ? WHERE id = ?",
            [status, result, at, elapsed, id]
          )

          if action_id do
            run(conn, @finish, [action_status(status), is_error, elapsed, action_id])
          end

          run(
            conn,
            "INSERT INTO outbox (job_id, intent, status, elapsed_ms, result, created_at, acked) VALUES (?, ?, ?, ?, ?, ?, 0)",
            [id, intent, status, elapsed, result, at]
          )

          {:ok, outbox_id} = Sqlite3.last_insert_rowid(conn)
          :ok = Sqlite3.execute(conn, "COMMIT")

          {:notify,
           %{
             id: outbox_id,
             job_id: id,
             intent: intent,
             status: status_atom(status),
             elapsed_ms: elapsed,
             result: result
           }}
      end

    {:reply, reply, state}
  end

  def handle_call({:job, id}, _from, %{conn: conn} = state) do
    reply =
      case fetch(conn, @select_job <> " WHERE id = ?", [id]) do
        [] -> nil
        [row] -> job_row_to_map(row)
      end

    {:reply, reply, state}
  end

  def handle_call({:job_output, id}, _from, %{conn: conn} = state) do
    rows = fetch(conn, "SELECT chunk FROM job_output WHERE job_id = ? ORDER BY seq", [id])
    {:reply, Enum.map_join(rows, "", fn [chunk] -> chunk end), state}
  end

  def handle_call({:recent_jobs, session_id, n}, _from, %{conn: conn} = state) do
    rows =
      fetch(conn, @select_job <> " WHERE session_id IS ? ORDER BY rowid DESC LIMIT ?", [
        session_id,
        n
      ])

    {:reply, Enum.map(rows, &job_row_to_map/1), state}
  end

  def handle_call(:unacked_outbox, _from, %{conn: conn} = state) do
    rows =
      fetch(
        conn,
        "SELECT id, job_id, intent, status, elapsed_ms, result FROM outbox WHERE acked = 0 ORDER BY id",
        []
      )

    {:reply, Enum.map(rows, &outbox_row_to_map/1), state}
  end

  def handle_call({:ack_outbox, ids}, _from, %{conn: conn} = state) do
    Enum.each(ids, fn id -> run(conn, "UPDATE outbox SET acked = 1 WHERE id = ?", [id]) end)
    {:reply, :ok, state}
  end

  defp ensure_version(conn) do
    case user_version(conn) do
      @user_version ->
        :ok

      found when found > @user_version ->
        {:future, found}

      0 ->
        # Every database written before stamping existed reads 0, so a 0 is
        # classified by inspecting the actual tables -- the one situation
        # where sniffing is sound, because 0 can only be a fresh file or a
        # shape older than stamping -- and the file leaves stamped, so every
        # later open trusts the version alone.
        case unstamped_version(conn) do
          :fresh -> create(conn)
          version -> migrate(conn, version)
        end

      found ->
        migrate(conn, found)
    end
  end

  defp user_version(conn) do
    [[version]] = fetch(conn, "PRAGMA user_version", [])
    version
  end

  defp unstamped_version(conn) do
    case table_columns(conn, "actions") do
      [] ->
        :fresh

      columns ->
        cond do
          "session_id" not in columns -> 1
          "status" not in columns -> 2
          "line" not in columns -> 3
          not table_exists?(conn, "jobs") -> 4
          true -> @user_version
        end
    end
  end

  defp table_exists?(conn, table) do
    fetch(conn, "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?", [table]) != []
  end

  defp create(conn) do
    execute_all(
      conn,
      [
        "BEGIN IMMEDIATE",
        @create_sessions,
        @create_topics,
        @create_actions,
        @create_jobs,
        @create_job_output,
        @create_outbox,
        stamp(),
        "COMMIT"
      ]
    )
  end

  # An already-current file from before stamping existed: mark it, move on.
  defp migrate(conn, @user_version), do: execute_all(conn, [stamp()])

  defp migrate(conn, from) do
    @migrations
    |> Enum.drop_while(fn {version, _statements} -> version < from end)
    |> Enum.each(fn {version, statements} ->
      execute_all(conn, ["BEGIN IMMEDIATE"] ++ statements ++ [stamp(version + 1), "COMMIT"])
    end)
  end

  defp stamp(version \\ @user_version), do: "PRAGMA user_version = #{version}"

  defp disabled_reply({:create_session, _name, _at}), do: 0
  defp disabled_reply({:create_topic, _session_id, _name, _at}), do: 0
  defp disabled_reply({:start_action, _action}), do: 0
  defp disabled_reply({:finish_job, _id, _status, _result, _at}), do: :already_final
  defp disabled_reply({:job, _id}), do: nil
  defp disabled_reply({:job_output, _id}), do: ""
  defp disabled_reply({:recent_jobs, _session_id, _n}), do: []
  defp disabled_reply(:unacked_outbox), do: []
  defp disabled_reply({:recent, _n}), do: []
  defp disabled_reply(:sessions), do: []
  defp disabled_reply(:topics), do: []
  defp disabled_reply(_request), do: :ok

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

  defp state_home do
    System.get_env("XDG_STATE_HOME") || Path.join(System.user_home!(), ".local/state")
  end

  defp now, do: DateTime.utc_now() |> DateTime.to_iso8601()

  defp bool_to_int(true), do: 1
  defp bool_to_int(false), do: 0

  defp elapsed_ms(started_at, at) do
    case {DateTime.from_iso8601(started_at), DateTime.from_iso8601(at)} do
      {{:ok, started, _}, {:ok, finished, _}} ->
        max(DateTime.diff(finished, started, :millisecond), 0)

      _ ->
        0
    end
  end

  # A killed or crashed job's action row is a failure; cancelled stays
  # cancelled, done stays done. The actions vocabulary has no 'killed'.
  defp action_status("killed"), do: "failed"
  defp action_status(status), do: status

  defp status_atom("running"), do: :running
  defp status_atom("done"), do: :done
  defp status_atom("failed"), do: :failed
  defp status_atom("cancelled"), do: :cancelled
  defp status_atom("killed"), do: :killed

  defp job_row_to_map([
         id,
         intent,
         session,
         topic,
         code,
         status,
         watch,
         result,
         output_bytes,
         output_dropped,
         started_at,
         finished_at,
         elapsed_ms
       ]) do
    %{
      id: id,
      intent: intent,
      session: session,
      topic: topic,
      code: code,
      status: status_atom(status),
      watch: watch == 1,
      result: result,
      output_bytes: output_bytes,
      output_dropped: output_dropped,
      started_at: started_at,
      finished_at: finished_at,
      elapsed_ms: elapsed_ms
    }
  end

  defp outbox_row_to_map([id, job_id, intent, status, elapsed_ms, result]) do
    %{
      id: id,
      job_id: job_id,
      intent: intent,
      status: status_atom(status),
      elapsed_ms: elapsed_ms,
      result: result
    }
  end

  defp row_to_entry([
         at,
         session,
         topic,
         tool,
         intent,
         arguments,
         is_error,
         elapsed_ms,
         status,
         stack,
         line
       ]) do
    %{
      at: at,
      session: session,
      topic: topic,
      tool: tool,
      intent: intent,
      arguments: arguments,
      is_error: is_error == 1,
      elapsed_ms: elapsed_ms,
      status: status,
      stack: stack,
      line: line
    }
  end
end
