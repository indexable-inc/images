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
  supervisor reopens the log -- except transient `SQLITE_BUSY` (#3874):
  several server instances share this database, so a sibling holding the
  write lock is normal operation, not a fault. sqlite itself waits it out
  (`PRAGMA busy_timeout`) and a bounded retry covers the rest; before that,
  one busy write match-crashed the GenServer, the exit propagated into
  whichever process sat in `GenServer.call` (a job's output flush, an
  exec's `start_action`), and under sustained contention the crash loop
  could exhaust the root supervisor's restart intensity and take the whole
  kernel -- and every running job -- down with it. The client API absorbs
  the restart blip too: `call/3` retries a call that died with the server,
  so callers survive an ActionLog restart instead of inheriting its exit.
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

  # index#3880: the arbiter for issue pickup. Every kernel instance on a host
  # shares this database, so the UNIQUE(repo, number) constraint IS the
  # atomic claim: the winning INSERT gets the row, every later attempt reads
  # the winner back. Known limit, by design: the database is per host, so
  # cross-machine claims race; the GitHub assignee mirror in `IxMcp.Issues`
  # is not compare-and-set, so it stays a mirror, not the arbiter.
  @create_issue_claims """
  CREATE TABLE issue_claims (id INTEGER PRIMARY KEY, repo TEXT NOT NULL, number INTEGER NOT NULL, session_id INTEGER REFERENCES sessions(id), claimed_at TEXT NOT NULL, UNIQUE(repo, number))
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
  # 4 = the #3546 live cell line, 5 = the #3839 durable job ledger,
  # 6 = the #3880 issue-claim arbiter.
  @user_version 6

  # SQLITE_BUSY tolerance (#3874). The pragma makes sqlite wait out a
  # sibling instance's write lock inside the NIF; the bounded retry on top
  # covers the starvation window where the timeout still expires. Only
  # after both does a write crash loudly (a stuck-forever database is a
  # real fault, not contention).
  @busy_timeout_ms Application.compile_env(:ix_mcp, :busy_timeout_ms, 5_000)
  @busy_retries 3
  @busy_retry_sleep_ms 100

  # How long the client API keeps retrying a call whose server died
  # mid-request or is restarting (#3874). The supervisor brings the log
  # back within milliseconds; this only needs to outlive the blip.
  @call_retries 20
  @call_retry_sleep_ms 100

  # Callers must outwait the server's own worst case, not the default 5s:
  # one statement can legitimately occupy the single-writer GenServer for
  # busy_timeout x retries (a slow sibling holding the lock), and a caller
  # that times out at exactly @busy_timeout_ms would die with the same
  # symptom #3874 fixed, just as a timeout-exit instead of a crash-exit.
  @call_timeout_ms 30_000

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

  # A v5 database predates the issue-claim arbiter (#3880): the table is
  # simply created empty.
  @migrate_v5_to_v6 [@create_issue_claims]

  # Ordered migrations keyed by the user_version each upgrades FROM. Every
  # step runs in one immediate transaction that also stamps the version it
  # produces, so an interrupted migration leaves the previous consistent,
  # correctly-stamped version on disk.
  @migrations [
    {1, @migrate_v1_to_v2},
    {2, @migrate_v2_to_v3},
    {3, @migrate_v3_to_v4},
    {4, @migrate_v4_to_v5},
    {5, @migrate_v5_to_v6}
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

  @select_issue_claim """
  SELECT c.id, c.repo, c.number, c.session_id, s.name, c.claimed_at
  FROM issue_claims c
  LEFT JOIN sessions s ON s.id = c.session_id
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

  @typedoc """
  A recorded issue claim (#3880): `session` is the claiming sessions row's
  name (nil when that session never named itself).
  """
  @type issue_claim :: %{
          id: integer(),
          repo: String.t(),
          number: integer(),
          session_id: integer() | nil,
          session: String.t() | nil,
          claimed_at: String.t()
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
    call(server, {:create_session, name, now()})
  end

  @doc "Set an existing session row's name."
  @spec rename_session(integer(), String.t(), GenServer.server()) :: :ok
  def rename_session(id, name, server \\ __MODULE__) do
    call(server, {:rename_session, id, name})
  end

  @doc "Insert a topics row under `session_id`; returns its id."
  @spec create_topic(integer(), String.t(), GenServer.server()) :: integer()
  def create_topic(session_id, name, server \\ __MODULE__) do
    call(server, {:create_topic, session_id, name, now()})
  end

  @doc "Insert an action row as `running` before the call executes; returns its id."
  @spec start_action(map(), GenServer.server()) :: integer()
  def start_action(action, server \\ __MODULE__) do
    call(server, {:start_action, Map.put(action, :at, now())})
  end

  @doc "Finalize a running action row; a no-op when it already finished."
  @spec finish_action(integer(), String.t(), boolean(), non_neg_integer(), GenServer.server()) ::
          :ok
  def finish_action(id, status, is_error, elapsed_ms, server \\ __MODULE__)
      when status in ["done", "failed", "cancelled"] do
    call(server, {:finish_action, id, status, is_error, elapsed_ms})
  end

  @doc """
  Refresh a running action row's sampled stack (JSON frames) and the cell
  source line the eval currently sits on (nil when the sample has no frame
  the cell owns, #3546); a no-op once finished.
  """
  @spec update_stack(integer(), String.t(), pos_integer() | nil, GenServer.server()) :: :ok
  def update_stack(id, stack_json, line, server \\ __MODULE__) do
    call(server, {:update_stack, id, stack_json, line, now()})
  end

  @doc "Latest `n` recorded actions, newest first, with session/topic names joined in."
  @spec recent(pos_integer(), GenServer.server()) :: [entry()]
  def recent(n \\ 20, server \\ __MODULE__) do
    call(server, {:recent, n})
  end

  @doc "All sessions rows, oldest first."
  @spec sessions(GenServer.server()) :: [
          %{id: integer(), name: String.t() | nil, started_at: String.t()}
        ]
  def sessions(server \\ __MODULE__) do
    call(server, :sessions)
  end

  @doc "All topics rows, oldest first."
  @spec topics(GenServer.server()) ::
          [%{id: integer(), session_id: integer(), name: String.t(), started_at: String.t()}]
  def topics(server \\ __MODULE__) do
    call(server, :topics)
  end

  # -- durable job ledger (#3839) --------------------------------------------

  @doc "Insert a `jobs` row as `running` when a job starts; idempotent per id."
  @spec job_started(job_start(), GenServer.server()) :: :ok
  def job_started(job, server \\ __MODULE__) do
    call(server, {:job_started, job})
  end

  @doc """
  Append output `chunks` (`[{seq, chunk}]`) to a job, recording
  `dropped_total` -- the job's running total of bytes discarded by the
  per-job cap, an absolute value so a retried batch cannot double-count
  (#3874). Batched by the job process so the hot path is not one call per
  `put_chars`.
  """
  @spec append_job_output(
          String.t(),
          [{integer(), binary()}],
          non_neg_integer(),
          GenServer.server()
        ) ::
          :ok
  def append_job_output(id, chunks, dropped_total \\ 0, server \\ __MODULE__) do
    call(server, {:append_job_output, id, chunks, dropped_total})
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
    call(server, {:finish_job, id, Atom.to_string(status), result, now()})
  end

  @doc "The recorded job row, or nil."
  @spec job(String.t(), GenServer.server()) :: job() | nil
  def job(id, server \\ __MODULE__) do
    call(server, {:job, id})
  end

  @doc "The full recorded output of a job, from the durable table."
  @spec job_output(String.t(), GenServer.server()) :: binary()
  def job_output(id, server \\ __MODULE__) do
    call(server, {:job_output, id})
  end

  @doc "Recent jobs for a session (nil = all), newest first."
  @spec recent_jobs(integer() | nil, pos_integer(), GenServer.server()) :: [job()]
  def recent_jobs(session_id, n \\ 20, server \\ __MODULE__) do
    call(server, {:recent_jobs, session_id, n})
  end

  # -- issue-claim arbiter (#3880) --------------------------------------------

  @doc """
  Atomically claim `repo#number` for `session_id`. The shared database's
  UNIQUE(repo, number) is the arbiter: the winning insert returns
  `{:ok, claim}`, a conflict returns `{:error, winner}` with the standing
  claim (including the winning session's name) so the loser can say who got
  there first. `:disabled` when the log is degraded (#3539) -- with no
  arbiter there is no claim to win.
  """
  @spec claim_issue(String.t(), integer(), integer() | nil, GenServer.server()) ::
          {:ok, issue_claim()} | {:error, issue_claim()} | :disabled
  def claim_issue(repo, number, session_id, server \\ __MODULE__)
      when is_binary(repo) and is_integer(number) do
    GenServer.call(server, {:claim_issue, repo, number, session_id, now()})
  end

  @doc """
  Claims with id greater than `id`, oldest first. The cursor is the caller's
  own watermark, per instance on purpose: several kernel instances share this
  database and each must announce every claim to its own client, so a shared
  announced flag (first sweeper wins) would silence all but one (#3880).
  """
  @spec issue_claims_after(integer(), GenServer.server()) :: [issue_claim()]
  def issue_claims_after(id, server \\ __MODULE__) do
    GenServer.call(server, {:issue_claims_after, id})
  end

  @doc "The highest issue-claim id (0 when none): a fresh watermark for `issue_claims_after/2`."
  @spec last_issue_claim_id(GenServer.server()) :: integer()
  def last_issue_claim_id(server \\ __MODULE__) do
    GenServer.call(server, :last_issue_claim_id)
  end

  @doc """
  Unacked outbox rows (terminal-transition notifications), oldest first.
  Scoped to `session_id` when given -- replay must never touch a sibling
  instance's rows, the same isolation the shared database demands
  everywhere else (#3839). `nil` returns every unacked row (introspection).
  """
  @spec unacked_outbox(integer() | nil, GenServer.server()) :: [outbox()]
  def unacked_outbox(session_id \\ nil, server \\ __MODULE__) do
    call(server, {:unacked_outbox, session_id})
  end

  @doc """
  Claim outbox rows as delivered; returns how many this call actually
  flipped from unacked to acked. A zero flip means someone already delivered
  the row, which is how a racing publish and replay avoid a double announce.
  """
  @spec ack_outbox([integer()], GenServer.server()) :: non_neg_integer()
  def ack_outbox(ids, server \\ __MODULE__) do
    call(server, {:ack_outbox, ids})
  end

  # Every public function funnels through here (#3874). When the server dies
  # mid-request -- historically a SQLITE_BUSY badmatch under a sibling's
  # write lock -- the exit propagated into whichever process was calling:
  # job control processes died mid-flush and vanished from the registry,
  # and the reaper died and forgot every monitor it held. The supervisor
  # restarts the log within milliseconds, so a bounded retry absorbs the
  # blip. A timeout is not retried (the request may still be executing);
  # exhaustion re-raises the exit, keeping a truly-down log loud. A rare
  # side effect is acceptable double-logging: a retried insert whose first
  # attempt committed before the server died writes a second row (job-ledger
  # writes are idempotent by key; a duplicate session/action row is only a
  # cosmetic log artifact).
  defp call(server, request), do: call(server, request, @call_retries)

  defp call(server, request, attempts) do
    GenServer.call(server, request, @call_timeout_ms)
  catch
    :exit, {:timeout, _call} = reason ->
      exit(reason)

    :exit, reason ->
      if attempts > 0 do
        Process.sleep(@call_retry_sleep_ms)
        call(server, request, attempts - 1)
      else
        exit(reason)
      end
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
    :ok = Sqlite3.execute(conn, "PRAGMA busy_timeout = #{@busy_timeout_ms}")

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

    :ok = step!(conn, insert)
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

  def handle_call({:append_job_output, id, chunks, dropped_total}, _from, %{conn: conn} = state) do
    :ok = execute!(conn, "BEGIN IMMEDIATE")

    # A batch can arrive twice (#3874): the job retries a flush whose reply
    # was lost, and the client seam retries a call whose server died after
    # committing. The rows are idempotent by (job_id, seq); the counters
    # must be too, so bytes count only rows this insert actually added and
    # the drop total is an absolute high-water mark, not a delta.
    added =
      Enum.reduce(chunks, 0, fn {seq, chunk}, acc ->
        run(conn, "INSERT OR IGNORE INTO job_output (job_id, seq, chunk) VALUES (?, ?, ?)", [
          id,
          seq,
          chunk
        ])

        {:ok, inserted} = Sqlite3.changes(conn)
        acc + inserted * byte_size(chunk)
      end)

    run(
      conn,
      "UPDATE jobs SET output_bytes = output_bytes + ?, output_dropped = MAX(output_dropped, ?) WHERE id = ?",
      [added, dropped_total, id]
    )

    :ok = execute!(conn, "COMMIT")
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
          :ok = execute!(conn, "BEGIN IMMEDIATE")

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
          :ok = execute!(conn, "COMMIT")

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

  # The INSERT OR IGNORE plus the changes count is the whole race (#3880):
  # a unique-constraint winner changes one row, every loser changes zero and
  # reads the winner back. The single-writer GenServer serializes claims from
  # this instance; claims from sibling instances serialize on SQLite itself.
  def handle_call({:claim_issue, repo, number, session_id, at}, _from, %{conn: conn} = state) do
    run(
      conn,
      "INSERT OR IGNORE INTO issue_claims (repo, number, session_id, claimed_at) VALUES (?, ?, ?, ?)",
      [repo, number, session_id, at]
    )

    {:ok, changes} = Sqlite3.changes(conn)

    [row] =
      fetch(conn, @select_issue_claim <> " WHERE c.repo = ? AND c.number = ?", [repo, number])

    claim = issue_claim_row_to_map(row)
    {:reply, if(changes == 1, do: {:ok, claim}, else: {:error, claim}), state}
  end

  def handle_call({:issue_claims_after, id}, _from, %{conn: conn} = state) do
    rows = fetch(conn, @select_issue_claim <> " WHERE c.id > ? ORDER BY c.id", [id])
    {:reply, Enum.map(rows, &issue_claim_row_to_map/1), state}
  end

  def handle_call(:last_issue_claim_id, _from, %{conn: conn} = state) do
    [[id]] = fetch(conn, "SELECT COALESCE(MAX(id), 0) FROM issue_claims", [])
    {:reply, id, state}
  end

  def handle_call({:unacked_outbox, session_id}, _from, %{conn: conn} = state) do
    {sql, params} =
      case session_id do
        nil ->
          {"SELECT id, job_id, intent, status, elapsed_ms, result FROM outbox WHERE acked = 0 ORDER BY id",
           []}

        sid ->
          {"SELECT o.id, o.job_id, o.intent, o.status, o.elapsed_ms, o.result FROM outbox o " <>
             "JOIN jobs j ON j.id = o.job_id WHERE j.session_id IS ? AND o.acked = 0 ORDER BY o.id",
           [sid]}
      end

    {:reply, Enum.map(fetch(conn, sql, params), &outbox_row_to_map/1), state}
  end

  # Claim each row that is still unacked; the count of rows this call flips is
  # the arbiter that keeps a racing publish and replay from double-announcing
  # (#3839). The SELECT-then-UPDATE is atomic here because it runs inside one
  # call on the single-writer GenServer.
  def handle_call({:ack_outbox, ids}, _from, %{conn: conn} = state) do
    claimed =
      Enum.reduce(ids, 0, fn id, acc ->
        case fetch(conn, "SELECT acked FROM outbox WHERE id = ?", [id]) do
          [[0]] ->
            run(conn, "UPDATE outbox SET acked = 1 WHERE id = ?", [id])
            acc + 1

          _ ->
            acc
        end
      end)

    {:reply, claimed, state}
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
          not table_exists?(conn, "issue_claims") -> 5
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
        @create_issue_claims,
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
  defp disabled_reply({:claim_issue, _repo, _number, _session_id, _at}), do: :disabled
  defp disabled_reply({:issue_claims_after, _id}), do: []
  defp disabled_reply(:last_issue_claim_id), do: 0
  defp disabled_reply({:job, _id}), do: nil
  defp disabled_reply({:job_output, _id}), do: ""
  defp disabled_reply({:recent_jobs, _session_id, _n}), do: []
  defp disabled_reply({:unacked_outbox, _session_id}), do: []
  defp disabled_reply({:ack_outbox, _ids}), do: 0
  defp disabled_reply({:recent, _n}), do: []
  defp disabled_reply(:sessions), do: []
  defp disabled_reply(:topics), do: []
  defp disabled_reply(_request), do: :ok

  defp table_columns(conn, table) do
    for [_cid, name | _rest] <- fetch(conn, "PRAGMA table_info(#{table})", []), do: name
  end

  defp execute_all(conn, statements) do
    Enum.each(statements, fn statement -> :ok = execute!(conn, statement) end)
  end

  defp run(conn, sql, params) do
    {:ok, statement} = Sqlite3.prepare(conn, sql)
    :ok = Sqlite3.bind(statement, params)
    :ok = step!(conn, statement)
    :ok = Sqlite3.release(conn, statement)
  end

  defp fetch(conn, sql, params) do
    {:ok, statement} = Sqlite3.prepare(conn, sql)
    :ok = Sqlite3.bind(statement, params)
    rows = fetch_all!(conn, statement)
    :ok = Sqlite3.release(conn, statement)
    rows
  end

  # -- SQLITE_BUSY tolerance (#3874) ------------------------------------------
  # sqlite already waits `@busy_timeout_ms` inside the NIF before reporting
  # busy; these bounded retries only cover the starvation window where that
  # timeout still expires under a slow sibling. Anything else stays a loud
  # crash, unchanged.

  defp step!(conn, statement), do: step!(conn, statement, @busy_retries)

  defp step!(conn, statement, attempts) do
    case Sqlite3.step(conn, statement) do
      :done ->
        :ok

      :busy when attempts > 0 ->
        Process.sleep(@busy_retry_sleep_ms)
        step!(conn, statement, attempts - 1)

      :busy ->
        raise "action log write still blocked after the busy-timeout wait and retries (#3890)"

      other ->
        raise "action log write failed: #{inspect(other)}"
    end
  end

  defp execute!(conn, sql), do: execute!(conn, sql, @busy_retries)

  defp execute!(conn, sql, attempts) do
    case Sqlite3.execute(conn, sql) do
      :ok ->
        :ok

      {:error, reason} when attempts > 0 ->
        if busy?(reason) do
          Process.sleep(@busy_retry_sleep_ms)
          execute!(conn, sql, attempts - 1)
        else
          raise "action log execute failed: #{inspect(reason)}"
        end

      {:error, reason} ->
        raise "action log execute failed: #{inspect(reason)}"
    end
  end

  defp fetch_all!(conn, statement), do: fetch_all!(conn, statement, @busy_retries)

  defp fetch_all!(conn, statement, attempts) do
    case Sqlite3.fetch_all(conn, statement) do
      {:ok, rows} ->
        rows

      {:error, reason} when attempts > 0 ->
        if busy?(reason) do
          Process.sleep(@busy_retry_sleep_ms)
          fetch_all!(conn, statement, attempts - 1)
        else
          raise "action log read failed: #{inspect(reason)}"
        end

      {:error, reason} ->
        raise "action log read failed: #{inspect(reason)}"
    end
  end

  defp busy?(reason) when is_binary(reason) do
    reason =~ "database is locked" or reason =~ "database table is locked"
  end

  defp busy?(_reason), do: false

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

  defp issue_claim_row_to_map([id, repo, number, session_id, session, claimed_at]) do
    %{
      id: id,
      repo: repo,
      number: number,
      session_id: session_id,
      session: session,
      claimed_at: claimed_at
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
