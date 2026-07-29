defmodule IxMcp.ActionLog do
  @moduledoc """
  Append-only SQLite record of every MCP action (#3512), normalized (#3532):
  a `sessions` row per server instance (created lazily on first use, so a
  connection that never acts leaves no row), a `topics` row per topic (a
  timeline -- repeating a name makes a new row), and an `actions` row
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
  supervisor reopens the log -- with transient `SQLITE_BUSY` waited out
  first (#3874/#3890): several server instances share this database, so a
  sibling holding the write lock is normal operation, not a fault. The wait
  is a wall-clock deadline paced by short NIF-level waits and scheduler-free
  sleeps (#3903), and only a lock outliving that budget crashes, with a
  diagnosis instead of a bare badmatch. Before that, one busy write
  match-crashed the GenServer, the
  exit propagated into whichever process sat in `GenServer.call` (a job's
  output flush, an exec's `start_action`), and under sustained contention
  the crash loop could exhaust the root supervisor's restart intensity and
  take the whole kernel -- and every running job -- down with it. The
  client API absorbs the restart blip too: `call/3` retries a call that
  died with the server, so callers survive an ActionLog restart instead of
  inheriting its exit.
  """

  use GenServer

  alias Exqlite.Sqlite3
  alias IxMcp.Jobs.Job

  require Logger

  # The schema is a published contract (#3532): the action-log UI is built
  # against these exact tables, so changes here must be coordinated.
  @create_sessions """
  CREATE TABLE sessions (id INTEGER PRIMARY KEY, name TEXT, started_at TEXT NOT NULL, last_seen_at TEXT)
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

  # The v6 shape of the issue-claim arbiter (#3880), frozen for the 5 -> 6
  # migration step: #3883 generalized it into `requests` (v7 -> 8 migrates
  # the rows across and drops this table), so history stays history.
  @create_issue_claims_v6 """
  CREATE TABLE issue_claims (id INTEGER PRIMARY KEY, repo TEXT NOT NULL, number INTEGER NOT NULL, session_id INTEGER REFERENCES sessions(id), claimed_at TEXT NOT NULL, UNIQUE(repo, number))
  """

  # index#3883: the request bus, generalizing issue pickup (#3880) to any
  # unit of work an agent can offer ("review this diff", "run this eval").
  # kind is 'issue' (ref = "owner/repo#n") or 'adhoc' (ref NULL); status
  # walks open -> claimed -> done. Claiming is a single UPDATE guarded on
  # status = 'open' -- the row count decides the race -- and the UNIQUE ref
  # makes ensuring an issue-kind row idempotent (SQLite unique treats NULLs
  # as distinct, so adhoc rows never collide). Known limit, by design: the
  # database is per host, so cross-machine claims race; the GitHub assignee
  # mirror in `IxMcp.Issues` is not compare-and-set, so it stays a mirror,
  # not the arbiter.
  @create_requests """
  CREATE TABLE requests (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, ref TEXT UNIQUE, title TEXT NOT NULL, body TEXT, posted_by INTEGER REFERENCES sessions(id), status TEXT NOT NULL DEFAULT 'open', claimed_by INTEGER REFERENCES sessions(id), posted_at TEXT NOT NULL, claimed_at TEXT, done_at TEXT)
  """

  # Append-only companion to `requests` (#3883): one row per mutation
  # (posted/claimed/done), written in the same transaction, so the feed each
  # instance sweeps (`IxMcp.SessionWatch`) can never see a state the table
  # does not hold. The requests row is current state; this is its history.
  @create_request_events """
  CREATE TABLE request_events (id INTEGER PRIMARY KEY, request_id INTEGER NOT NULL REFERENCES requests(id), event TEXT NOT NULL, session_id INTEGER REFERENCES sessions(id), at TEXT NOT NULL)
  """

  # index#3881: the per-host bus between kernel instances. A NULL to_session
  # is a broadcast. Delivery is each instance's own business: `IxMcp.SessionWatch`
  # sweeps rows addressed to its session (or to everyone) past a per-instance
  # watermark -- the claim-feed cursor pattern (#3880), and per instance for
  # the same reason: every instance must deliver to its own client.
  @create_session_messages """
  CREATE TABLE session_messages (id INTEGER PRIMARY KEY, from_session INTEGER NOT NULL REFERENCES sessions(id), to_session INTEGER REFERENCES sessions(id), body TEXT NOT NULL, created_at TEXT NOT NULL)
  """

  # ENG-11209: fleet notification state, in two tables because the two things
  # have different lifetimes. A mute is an operator decision and must outlive
  # every reconnect -- muting something that un-mutes when the client
  # reconnects is not a mute. A seen fingerprint is dedup bookkeeping: it is
  # what makes a condition that stays true announce once instead of once per
  # poll, so it must be durable for the same reason the outbox is (#3839) --
  # kept in memory, a kernel restart re-announces every standing fault.
  @create_fleet_mutes """
  CREATE TABLE fleet_mutes (predicate TEXT PRIMARY KEY, muted_at TEXT NOT NULL, reason TEXT)
  """

  @create_fleet_alerts_seen """
  CREATE TABLE fleet_alerts_seen (fingerprint TEXT PRIMARY KEY, predicate TEXT NOT NULL, summary TEXT, first_seen_at TEXT NOT NULL, last_seen_at TEXT NOT NULL)
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
  # 6 = the #3880 issue-claim arbiter, 7 = the #3881 session heartbeat and
  # message bus, 8 = the #3883 request bus (issue_claims folded in and
  # dropped), 9 = the ENG-11209 fleet notification state.
  @user_version 9

  # How long a statement waits for a sibling instance's write lock before
  # step!/fetch/execute! give up and crash with a diagnosis (#3890). A
  # wall-clock deadline enforced in Elixir, not sqlite's own busy handler:
  # exqlite runs statements on the BEAM's dirty IO schedulers, so a wait
  # spent inside the NIF holds one of those ~10 slots for its whole
  # duration. Stack a few concurrent waiters (the test suite runs many
  # instances in one BEAM; a loaded host does the same across kernels) and
  # the pool starves out the very COMMIT/ROLLBACK calls that would release
  # the lock -- observed as waits sailing past every configured bound
  # (#3903). Each NIF call therefore waits only @busy_nif_wait_ms inside
  # sqlite, and the long horizon is scheduler-free Process.sleep between
  # attempts.
  @busy_timeout_ms 5_000

  # Per-attempt busy wait inside the NIF: long enough to ride out
  # micro-contention without surfacing, short enough that a blocked
  # statement never camps on a dirty IO scheduler.
  @busy_nif_wait_ms 50

  # Scheduler-free pause between attempts once a NIF-level wait expired.
  @busy_poll_ms 25

  # How long the client API keeps retrying a call whose server died
  # mid-request or is restarting (#3874). The supervisor brings the log
  # back within milliseconds; this only needs to outlive the blip.
  @call_retries 20
  @call_retry_sleep_ms 100

  # Callers must outwait the server's own worst case, not the default 5s:
  # one statement can legitimately sit out the full busy-timeout wait
  # inside the single-writer GenServer (a slow sibling holding the lock),
  # and a caller that times out at exactly @busy_timeout_ms would die with
  # the same symptom #3874 fixed, just as a timeout-exit instead of a
  # crash-exit.
  @call_timeout_ms 30_000

  # index#3903: the flaky-test incident was exactly these two bounds being
  # equal -- the caller's timeout-exit always won the race against the
  # server's descriptive raise, so a blocked write reported as a bare
  # GenServer.call timeout instead of naming the blocked statement. The
  # default busy wait must end comfortably before the default call bound
  # (instances that override :busy_timeout_ms own that margin themselves).
  if @busy_timeout_ms >= @call_timeout_ms do
    raise CompileError,
      description: "@busy_timeout_ms must stay below @call_timeout_ms (index#3903)"
  end

  # Frozen historical DDL for the 1 -> 2 step: the actions shape exactly as
  # #3532 shipped it, before the live-row columns. A migration must never
  # borrow the current @create_actions, or editing today's schema would
  # silently rewrite the ladder's history.
  @create_actions_v2 """
  CREATE TABLE actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id), topic_id INTEGER REFERENCES topics(id), tool TEXT NOT NULL, intent TEXT, arguments TEXT NOT NULL, is_error INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL)
  """

  # Same freeze for sessions: the shape #3532 shipped, before the #3881
  # last_seen_at heartbeat column (which the 6 -> 7 step adds).
  @create_sessions_v2 """
  CREATE TABLE sessions (id INTEGER PRIMARY KEY, name TEXT, started_at TEXT NOT NULL)
  """

  # The v1 shape kept session/topic as TEXT per action row. The backfill
  # makes one session per distinct v1 session string (NULLs collapse into
  # one unnamed session, hence the NULL-safe `IS` joins), one topic per
  # distinct (session, topic) pair -- v1 rows carry no topic boundaries, so
  # per-pair is the finest lossless grain -- and earliest-seen timestamps
  # stand in for the started_at v1 never stored.
  @migrate_v1_to_v2 [
    "ALTER TABLE actions RENAME TO actions_v1",
    @create_sessions_v2,
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
  @migrate_v5_to_v6 [@create_issue_claims_v6]

  # A v6 database predates the session heartbeat and message bus (#3881):
  # existing sessions gain a NULL last_seen_at (they never heartbeat), and
  # the messages table is created empty.
  @migrate_v6_to_v7 [
    "ALTER TABLE sessions ADD COLUMN last_seen_at TEXT",
    @create_session_messages
  ]

  # A v7 database holds its issue claims in the #3880 table; the request bus
  # (#3883) subsumes them, so each claim becomes a claimed request of kind
  # 'issue' (ref doubles as the title -- the claim never carried one) and
  # the old table drops. No request_events rows are synthesized: the feed
  # announces news, and a migrated claim is history every instance already
  # heard as event="picked_up".
  @migrate_v7_to_v8 [
    @create_requests,
    @create_request_events,
    """
    INSERT INTO requests (kind, ref, title, posted_by, status, claimed_by, posted_at, claimed_at)
    SELECT 'issue', repo || '#' || number, repo || '#' || number, NULL, 'claimed', session_id, claimed_at, claimed_at
    FROM issue_claims ORDER BY id
    """,
    "DROP TABLE issue_claims"
  ]

  # A v8 database predates fleet notifications (ENG-11209): both tables are
  # created empty, which is exactly right -- an operator who has muted nothing
  # has no mutes, and an alert nobody has seen yet should announce on the first
  # poll after the upgrade.
  @migrate_v8_to_v9 [@create_fleet_mutes, @create_fleet_alerts_seen]

  # Ordered migrations keyed by the user_version each upgrades FROM. Every
  # step runs in one immediate transaction that also stamps the version it
  # produces, so an interrupted migration leaves the previous consistent,
  # correctly-stamped version on disk.
  @migrations [
    {1, @migrate_v1_to_v2},
    {2, @migrate_v2_to_v3},
    {3, @migrate_v3_to_v4},
    {4, @migrate_v4_to_v5},
    {5, @migrate_v5_to_v6},
    {6, @migrate_v6_to_v7},
    {7, @migrate_v7_to_v8},
    {8, @migrate_v8_to_v9}
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

  @select_request """
  SELECT r.id, r.kind, r.ref, r.title, r.body, r.posted_by, p.name, r.status, r.claimed_by, c.name, r.posted_at, r.claimed_at, r.done_at
  FROM requests r
  LEFT JOIN sessions p ON p.id = r.posted_by
  LEFT JOIN sessions c ON c.id = r.claimed_by
  """

  @select_request_event """
  SELECT e.id, e.request_id, e.event, e.session_id, s.name, e.at, r.kind, r.ref, r.title, r.body
  FROM request_events e
  JOIN requests r ON r.id = e.request_id
  LEFT JOIN sessions s ON s.id = e.session_id
  """

  @select_session_message """
  SELECT m.id, m.from_session, s.name, m.to_session, m.body, m.created_at
  FROM session_messages m
  LEFT JOIN sessions s ON s.id = m.from_session
  """

  # The latest topics row is the session's current topic: topics are a
  # timeline (#3532), so the newest row per session is what it works on now.
  @select_directory """
  SELECT s.id, s.name,
         (SELECT t.name FROM topics t WHERE t.session_id = s.id ORDER BY t.id DESC LIMIT 1),
         s.started_at, s.last_seen_at
  FROM sessions s ORDER BY s.id
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
  A request on the per-host work bus (#3883): `poster`/`claimer` are the
  posting/claiming sessions rows' names (nil when unnamed or, for `poster`,
  when the row was migrated from an issue claim, which nobody posted).
  """
  @type request :: %{
          id: integer(),
          kind: :issue | :adhoc,
          ref: String.t() | nil,
          title: String.t(),
          body: String.t() | nil,
          posted_by: integer() | nil,
          poster: String.t() | nil,
          status: :open | :claimed | :done,
          claimed_by: integer() | nil,
          claimer: String.t() | nil,
          posted_at: String.t(),
          claimed_at: String.t() | nil,
          done_at: String.t() | nil
        }

  @typedoc """
  A request mutation (#3883), joined with its request's identity so the feed
  can announce without a second read: `session` is the acting sessions row's
  name.
  """
  @type request_event :: %{
          id: integer(),
          request_id: integer(),
          event: :posted | :claimed | :done,
          session_id: integer() | nil,
          session: String.t() | nil,
          at: String.t(),
          kind: :issue | :adhoc,
          ref: String.t() | nil,
          title: String.t(),
          body: String.t() | nil
        }

  @typedoc """
  A cross-session message (#3881): `from` is the sending sessions row's name
  (nil when that session never named itself); a nil `to_session` is a
  broadcast.
  """
  @type session_message :: %{
          id: integer(),
          from_session: integer(),
          from: String.t() | nil,
          to_session: integer() | nil,
          body: String.t(),
          created_at: String.t()
        }

  @typedoc """
  A session-directory row (#3881): `topic` is the session's latest topics
  row, `last_seen_at` its heartbeat (nil = it never heartbeat: a pre-#3881
  row or an instance that never ran the watch).
  """
  @type directory_entry :: %{
          id: integer(),
          name: String.t() | nil,
          topic: String.t() | nil,
          started_at: String.t(),
          last_seen_at: String.t() | nil
        }

  @typedoc """
  A terminal-transition notification awaiting delivery (#3839). `session_id`
  is the owning job's session: live delivery is scoped to it, exactly like
  replay (#3934). `acked` rides along so a row born delivered (a quiet
  wrapper's, #3934) is never published.
  """
  @type outbox :: %{
          id: integer(),
          job_id: String.t(),
          intent: String.t() | nil,
          status: Job.status(),
          elapsed_ms: non_neg_integer() | nil,
          result: String.t() | nil,
          session_id: integer() | nil,
          acked: boolean()
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

  `quiet: true` marks the job an await wrapper (#3934): a clean finish is a
  read of another job's terminal state, not news, so its outbox row is born
  acked -- on the record, never announced. Only `done` goes quiet; a
  wrapper's own failure or death still announces (the invariant).

  `start:` is the job's start metadata (the `job_started/2` map, #4082).
  With it, a missing jobs row is reconstructed inside this transition
  instead of no-opping as `:already_final`: under load a job's `job_started`
  write can be absorbed by the job's ledger seam (#3874), and before #4082
  that single lost write erased the whole run from the record -- finish and
  output had no row to land on, `Jobs.history` never listed the job, and
  the durability contract ("history survives even a crash or kill") broke.
  """
  @spec finish_job(String.t(), Job.status(), String.t() | nil, keyword(), GenServer.server()) ::
          {:notify, outbox()} | :already_final
  def finish_job(id, status, result, opts \\ [], server \\ __MODULE__)
      when status in [:done, :failed, :cancelled, :killed] do
    quiet = Keyword.get(opts, :quiet, false)
    start = Keyword.get(opts, :start)
    call(server, {:finish_job, id, Atom.to_string(status), result, quiet, now(), start})
  end

  @doc """
  Ack every unacked outbox row of `job_id`; returns the flipped count. The
  exec reply path calls this once its reply carries the job's outcome, so
  the finish is never announced twice (#3934). Suppression must not outrun
  delivery: this runs strictly after the reply is rendered, so a death
  before that point leaves the row unacked and the announcement fires.
  """
  @spec ack_job_outbox(String.t(), GenServer.server()) :: non_neg_integer()
  def ack_job_outbox(job_id, server \\ __MODULE__) when is_binary(job_id) do
    call(server, {:ack_job_outbox, job_id})
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

  # -- request bus (#3883) -----------------------------------------------------

  @doc """
  Record a request offering `title` to every agent on the host. `kind:
  :adhoc` (ref nil) always inserts a fresh open row; `kind: :issue` requires
  `ref` (`"owner/repo#n"`) and is an idempotent ensure -- the UNIQUE ref
  makes a re-post read the standing row back, whatever its status. A
  `posted` event lands in the same transaction only when the row is new: the
  feed announces offers, and an existing row was already offered. Returns
  `{:ok, request}`; `:disabled` when the log is degraded (#3539) -- with no
  shared database there is no bus to post on.

  Plain GenServer.call, not the retrying seam (#3874): an adhoc post is not
  idempotent, so a retry across a server restart could offer the same work
  twice (the same reason `send_session_message/4` does not retry).
  """
  @spec post_request(
          :issue | :adhoc,
          String.t() | nil,
          String.t(),
          String.t() | nil,
          integer() | nil,
          GenServer.server()
        ) :: {:ok, request()} | :disabled
  def post_request(kind, ref, title, body, session_id, server \\ __MODULE__)
      when is_binary(title) and
             ((kind == :adhoc and is_nil(ref)) or (kind == :issue and is_binary(ref))) do
    GenServer.call(
      server,
      {:post_request, Atom.to_string(kind), ref, title, body, session_id, now()}
    )
  end

  @doc """
  Atomically claim request `id` for `session_id`: one UPDATE guarded on
  `status = 'open'`, the row count deciding the race (#3883). The winner
  gets `{:ok, request}` and a `claimed` event in the same transaction; a
  loser reads the standing row back as `{:error, request}` (status and
  claimer included) so it can say who got there first. Idempotent per
  session: re-claiming a request this session already holds is
  `{:ok, request}`, which is what makes the call safe for the client seam
  to retry across a server restart (#3903). `{:error, :not_found}` when no
  such request exists; `:disabled` when the log is degraded (#3539) -- with
  no arbiter there is no claim to win.
  """
  @spec claim_request(integer(), integer() | nil, GenServer.server()) ::
          {:ok, request()} | {:error, request()} | {:error, :not_found} | :disabled
  def claim_request(id, session_id, server \\ __MODULE__) when is_integer(id) do
    call(server, {:claim_request, id, session_id, now()})
  end

  @doc """
  Mark claimed request `id` done: the same guarded-UPDATE shape as
  `claim_request/3`, on `status = 'claimed'`, with a `done` event in the
  same transaction. Finishing an already-done request is `{:ok, request}`
  (idempotent, so the client seam can retry, #3903); a still-open one is
  `{:error, request}` -- claim what you work, then finish it.
  """
  @spec finish_request(integer(), integer() | nil, GenServer.server()) ::
          {:ok, request()} | {:error, request()} | {:error, :not_found} | :disabled
  def finish_request(id, session_id, server \\ __MODULE__) when is_integer(id) do
    call(server, {:finish_request, id, session_id, now()})
  end

  @doc """
  Atomically claim the issue-kind request for `repo#number`, ensuring its
  row first (`IxMcp.Issues.pickup/1`'s arbiter, #3880 generalized by
  #3883): ensure + claim run in one transaction, and a row born here is
  claimed at birth with only a `claimed` event -- it was never on offer, so
  a `posted` announcement would offer work that is already taken. Same
  returns and per-session idempotency as `claim_request/3`.
  """
  @spec claim_issue(String.t(), integer(), integer() | nil, GenServer.server()) ::
          {:ok, request()} | {:error, request()} | :disabled
  def claim_issue(repo, number, session_id, server \\ __MODULE__)
      when is_binary(repo) and is_integer(number) do
    call(server, {:claim_issue, repo, number, session_id, now()})
  end

  @doc "Every request, open first (then claimed, then done), newest first within a status."
  @spec list_requests(GenServer.server()) :: [request()]
  def list_requests(server \\ __MODULE__) do
    call(server, :list_requests)
  end

  @doc """
  Request events with id greater than `id`, oldest first. The cursor is the
  caller's own watermark, per instance on purpose: several kernel instances
  share this database and each must announce every event to its own client,
  so a shared announced flag (first sweeper wins) would silence all but one
  (#3880).
  """
  @spec request_events_after(integer(), GenServer.server()) :: [request_event()]
  def request_events_after(id, server \\ __MODULE__) do
    call(server, {:request_events_after, id})
  end

  @doc "The highest request-event id (0 when none): a fresh watermark for `request_events_after/2`."
  @spec last_request_event_id(GenServer.server()) :: integer()
  def last_request_event_id(server \\ __MODULE__) do
    call(server, :last_request_event_id)
  end

  # -- session directory + message bus (#3881) --------------------------------

  @doc """
  Stamp `session_id`'s liveness heartbeat (`last_seen_at = now`). Stamped on
  transport register (`IxMcp.MCP.Notifier`) and on every `IxMcp.SessionWatch`
  tick, so the directory can tell a live instance from a dead one's row.
  """
  @spec heartbeat_session(integer(), GenServer.server()) :: :ok
  def heartbeat_session(session_id, server \\ __MODULE__) when is_integer(session_id) do
    GenServer.call(server, {:heartbeat_session, session_id, now()})
  end

  @doc """
  Every sessions row joined with its current topic and heartbeat, oldest
  first: the raw directory `IxMcp.Sessions.list/1` filters and flags.
  """
  @spec session_directory(GenServer.server()) :: [directory_entry()]
  def session_directory(server \\ __MODULE__) do
    GenServer.call(server, :session_directory)
  end

  @doc """
  Record a message from `from_session` to `to_session` (nil = broadcast).
  Returns the recorded message; `:disabled` when the log is degraded (#3539)
  -- with no shared database there is no bus to carry it.
  """
  @spec send_session_message(integer(), integer() | nil, String.t(), GenServer.server()) ::
          {:ok, session_message()} | :disabled
  def send_session_message(from_session, to_session, body, server \\ __MODULE__)
      when is_integer(from_session) and is_binary(body) do
    GenServer.call(server, {:send_session_message, from_session, to_session, body, now()})
  end

  @doc """
  Messages for `session_id` past the `id` watermark, oldest first: rows
  addressed to it plus broadcasts, never its own sends. The cursor is per
  instance for the same reason as `request_events_after/2`: every instance
  must deliver to its own client (#3880).
  """
  @spec session_messages_after(integer(), integer(), GenServer.server()) :: [session_message()]
  def session_messages_after(id, session_id, server \\ __MODULE__) do
    GenServer.call(server, {:session_messages_after, id, session_id})
  end

  @doc "The highest session-message id (0 when none): a fresh watermark for `session_messages_after/3`."
  @spec last_session_message_id(GenServer.server()) :: integer()
  def last_session_message_id(server \\ __MODULE__) do
    GenServer.call(server, :last_session_message_id)
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

  @doc """
  Mute fleet predicate `id` durably (ENG-11209). Survives reconnects and
  kernel restarts, because it lives in the same SQLite file as everything
  else: a mute that evaporates when the client reconnects is not a mute.
  Idempotent -- re-muting keeps the original `muted_at`.
  """
  @spec mute_fleet_predicate(String.t(), String.t() | nil, GenServer.server()) :: :ok
  def mute_fleet_predicate(id, reason \\ nil, server \\ __MODULE__) when is_binary(id) do
    call(server, {:mute_fleet_predicate, id, reason, now()})
  end

  @doc "Unmute fleet predicate `id`. Unmuting something never muted is `:ok`."
  @spec unmute_fleet_predicate(String.t(), GenServer.server()) :: :ok
  def unmute_fleet_predicate(id, server \\ __MODULE__) when is_binary(id) do
    call(server, {:unmute_fleet_predicate, id})
  end

  @doc "Every muted predicate id, oldest mute first."
  @spec fleet_mutes(GenServer.server()) :: [map()]
  def fleet_mutes(server \\ __MODULE__), do: call(server, :fleet_mutes)

  @doc """
  Record that `fingerprint` was observed, and say whether it is new.

  `true` means nobody has been told about this condition instance yet, so it
  should be announced; `false` means it is already known and must stay silent.
  One guarded INSERT decides it, so two instances polling the same database
  concurrently cannot both announce the same fault.
  """
  @spec fleet_alert_new?(String.t(), String.t(), String.t(), GenServer.server()) :: boolean()
  def fleet_alert_new?(fingerprint, predicate, summary, server \\ __MODULE__) do
    call(server, {:fleet_alert_seen, fingerprint, predicate, summary, now()})
  end

  @doc """
  Every alert instance this kernel has announced and not forgotten, newest
  first. This is the standing-state read: a condition that fired once and is
  still true appears here rather than being re-announced.
  """
  @spec fleet_alerts_seen(GenServer.server()) :: [map()]
  def fleet_alerts_seen(server \\ __MODULE__), do: call(server, :fleet_alerts_seen)

  @doc """
  Forget seen fingerprints, so a still-standing condition announces again.
  `:all` clears everything; a predicate id clears just that predicate.
  Returns the number of rows dropped.
  """
  @spec forget_fleet_alerts(:all | String.t(), GenServer.server()) :: integer()
  def forget_fleet_alerts(scope \\ :all, server \\ __MODULE__) do
    call(server, {:forget_fleet_alerts, scope})
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
      # Retry only name-addressed servers: the restarted singleton comes
      # back under the same name, while a pid-addressed instance (tests,
      # ad-hoc logs) is gone for good and retrying it is a futile 2s stall.
      if attempts > 0 and not is_pid(server) do
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

    # Several server instances share one database file, so a step can find a
    # sibling holding the write lock (#3890). The NIF-level wait stays short
    # on purpose -- a long in-NIF wait camps on a dirty IO scheduler and
    # starves the sibling's releasing COMMIT/ROLLBACK (#3903) -- while
    # step!/fetch/execute! wait out the full busy budget in Elixir and turn
    # a lock outliving it into a descriptive crash. The :busy_timeout_ms
    # option exists so the regression test can shrink that budget.
    :ok = Sqlite3.set_busy_timeout(conn, min(@busy_nif_wait_ms, busy_budget(opts)))
    db = %{conn: conn, busy_timeout_ms: busy_budget(opts)}

    ensure_wal(db)

    # index#3539: on 2026-07-17 a server binary match-crashed right here
    # against an action log written under a newer schema, and the failed
    # child took the whole application down -- every tool call died over a
    # log nothing on the hot path reads. Older on-disk versions migrate
    # forward; a future version (a newer server already ran against this
    # file) refuses loudly but keeps the server up, degraded to not
    # recording, so the blast radius stays scoped to the log itself.
    case ensure_version(db) do
      :ok ->
        {:ok, insert} = Sqlite3.prepare(conn, @insert)
        {:ok, %{db: db, insert: insert}}

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

  # A raise in a callback (a lock outliving the busy budget, #3890) restarts
  # this server, but the dead connection is a NIF resource: its file handle
  # -- and any RESERVED lock a transaction it began still holds -- survives
  # until the garbage collector reclaims the resource, which has no deadline.
  # Until then that orphaned lock can block the freshly restarted server and
  # every sibling instance on the shared database. Close explicitly on the
  # way down so the lock dies with the server. The degraded `:disabled`
  # state already closed its connection in init.
  @impl true
  def terminate(_reason, %{db: db}) do
    _ = Sqlite3.close(db.conn)
    :ok
  end

  def terminate(_reason, _state), do: :ok

  # The shared file must run in WAL mode (#4092): under the default rollback
  # journal one instance's long read -- a history scan over a grown log --
  # holds the lock every sibling's writes need, and on 2026-07-23 those
  # blocked writes outlived the busy budget, crash-looped every instance's
  # log, and killed two whole kernels through restart intensity. WAL keeps
  # readers and the single writer independent, leaving busy_wait to genuine
  # writer-writer contention. The pragma is persistent and converts an
  # existing rollback-journal file in place; conversion needs a moment of
  # exclusivity, so it rides the same busy budget as any write. Anything but
  # wal (or memory, for the ":memory:" test databases) means the conversion
  # failed and this instance would reintroduce the blocking mode for every
  # sibling, so it refuses to run rather than degrade silently.
  defp ensure_wal(db) do
    case fetch(db, "PRAGMA journal_mode=WAL", []) do
      [[mode]] when mode in ["wal", "memory"] ->
        :ok

      [[mode]] ->
        raise "action log could not enter WAL mode (#4092), still #{inspect(mode)}"
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

  def handle_call({:create_session, name, at}, _from, %{db: db} = state) do
    run(db, "INSERT INTO sessions (name, started_at) VALUES (?, ?)", [name, at])
    {:ok, id} = Sqlite3.last_insert_rowid(db.conn)
    {:reply, id, state}
  end

  def handle_call({:rename_session, id, name}, _from, %{db: db} = state) do
    run(db, "UPDATE sessions SET name = ? WHERE id = ?", [name, id])
    {:reply, :ok, state}
  end

  def handle_call({:create_topic, session_id, name, at}, _from, %{db: db} = state) do
    run(db, "INSERT INTO topics (session_id, name, started_at) VALUES (?, ?, ?)", [
      session_id,
      name,
      at
    ])

    {:ok, id} = Sqlite3.last_insert_rowid(db.conn)
    {:reply, id, state}
  end

  def handle_call({:start_action, action}, _from, %{db: db, insert: insert} = state) do
    :ok =
      Sqlite3.bind(insert, [
        action.at,
        action.session_id,
        action.topic_id,
        action.tool,
        action.intent,
        action.arguments
      ])

    :done = step!(db, insert, "insert action")
    {:ok, id} = Sqlite3.last_insert_rowid(db.conn)
    {:reply, id, state}
  end

  def handle_call(
        {:finish_action, id, status, is_error, elapsed_ms},
        _from,
        %{db: db} = state
      ) do
    run(db, @finish, [status, bool_to_int(is_error), elapsed_ms, id])
    {:reply, :ok, state}
  end

  def handle_call({:update_stack, id, stack_json, line, at}, _from, %{db: db} = state) do
    run(db, @update_stack, [stack_json, at, line, id])
    {:reply, :ok, state}
  end

  def handle_call({:recent, n}, _from, %{db: db} = state) do
    rows = fetch(db, @select_recent, [n])
    {:reply, Enum.map(rows, &row_to_entry/1), state}
  end

  def handle_call(:sessions, _from, %{db: db} = state) do
    rows =
      for [id, name, started_at] <-
            fetch(db, "SELECT id, name, started_at FROM sessions ORDER BY id", []) do
        %{id: id, name: name, started_at: started_at}
      end

    {:reply, rows, state}
  end

  def handle_call(:topics, _from, %{db: db} = state) do
    rows =
      for [id, session_id, name, started_at] <-
            fetch(db, "SELECT id, session_id, name, started_at FROM topics ORDER BY id", []) do
        %{id: id, session_id: session_id, name: name, started_at: started_at}
      end

    {:reply, rows, state}
  end

  def handle_call({:job_started, job}, _from, %{db: db} = state) do
    insert_job_row(db, job)
    {:reply, :ok, state}
  end

  def handle_call({:append_job_output, id, chunks, dropped_total}, _from, %{db: db} = state) do
    :ok = execute!(db, "BEGIN IMMEDIATE")

    # A batch can arrive twice (#3874): the job retries a flush whose reply
    # was lost, and the client seam retries a call whose server died after
    # committing. The rows are idempotent by (job_id, seq); the counters
    # must be too, so bytes count only rows this insert actually added and
    # the drop total is an absolute high-water mark, not a delta.
    added =
      Enum.reduce(chunks, 0, fn {seq, chunk}, acc ->
        run(db, "INSERT OR IGNORE INTO job_output (job_id, seq, chunk) VALUES (?, ?, ?)", [
          id,
          seq,
          chunk
        ])

        {:ok, inserted} = Sqlite3.changes(db.conn)
        acc + inserted * byte_size(chunk)
      end)

    run(
      db,
      "UPDATE jobs SET output_bytes = output_bytes + ?, output_dropped = MAX(output_dropped, ?) WHERE id = ?",
      [added, dropped_total, id]
    )

    :ok = execute!(db, "COMMIT")
    {:reply, :ok, state}
  end

  # The one atomic terminal transition (#3839): the SELECT is the idempotency
  # guard (only a still-running job transitions), and the jobs row, its
  # actions row, and the outbox insert all commit together, so a reader can
  # never catch the two logs disagreeing and no terminal transition escapes
  # the outbox. The single-writer GenServer serializes this against every
  # other write, so the guard needs no locking of its own.
  def handle_call({:finish_job, id, status, result, quiet, at, start}, _from, %{db: db} = state) do
    # Reconstruct a lost start row before the guarded transition (#4082):
    # idempotent by primary key, so a row that already exists -- running or
    # terminal -- is untouched and the guard below still decides the race.
    if is_map(start), do: insert_job_row(db, start)

    reply =
      case fetch(
             db,
             "SELECT action_id, intent, started_at, session_id FROM jobs WHERE id = ? AND status = 'running'",
             [id]
           ) do
        [] ->
          :already_final

        [[action_id, intent, started_at, session_id]] ->
          elapsed = elapsed_ms(started_at, at)
          is_error = if status in ["failed", "killed"], do: 1, else: 0

          # A quiet wrapper's clean finish is born acked (#3934): the row
          # stays on the record but no delivery path will ever pick it up.
          # Anything but `done` stays announceable -- a wrapper's own death
          # is still a death.
          acked = if quiet and status == "done", do: 1, else: 0
          :ok = execute!(db, "BEGIN IMMEDIATE")

          run(
            db,
            "UPDATE jobs SET status = ?, result = ?, finished_at = ?, elapsed_ms = ? WHERE id = ?",
            [status, result, at, elapsed, id]
          )

          if action_id do
            run(db, @finish, [action_status(status), is_error, elapsed, action_id])
          end

          run(
            db,
            "INSERT INTO outbox (job_id, intent, status, elapsed_ms, result, created_at, acked) VALUES (?, ?, ?, ?, ?, ?, ?)",
            [id, intent, status, elapsed, result, at, acked]
          )

          {:ok, outbox_id} = Sqlite3.last_insert_rowid(db.conn)
          :ok = execute!(db, "COMMIT")

          {:notify,
           %{
             id: outbox_id,
             job_id: id,
             intent: intent,
             status: status_atom(status),
             elapsed_ms: elapsed,
             result: result,
             session_id: session_id,
             acked: acked == 1
           }}
      end

    {:reply, reply, state}
  end

  def handle_call({:job, id}, _from, %{db: db} = state) do
    reply =
      case fetch(db, @select_job <> " WHERE id = ?", [id]) do
        [] -> nil
        [row] -> job_row_to_map(row)
      end

    {:reply, reply, state}
  end

  def handle_call({:job_output, id}, _from, %{db: db} = state) do
    rows = fetch(db, "SELECT chunk FROM job_output WHERE job_id = ? ORDER BY seq", [id])
    {:reply, Enum.map_join(rows, "", fn [chunk] -> chunk end), state}
  end

  def handle_call({:recent_jobs, session_id, n}, _from, %{db: db} = state) do
    rows =
      fetch(db, @select_job <> " WHERE session_id IS ? ORDER BY rowid DESC LIMIT ?", [
        session_id,
        n
      ])

    {:reply, Enum.map(rows, &job_row_to_map/1), state}
  end

  # The INSERT OR IGNORE plus the changes count is the ensure (#3883): the
  # UNIQUE ref makes re-posting an issue-kind request read the standing row
  # back, while an adhoc post (NULL ref, never unique-conflicting) always
  # inserts. Only a fresh row gets a posted event, in the same transaction:
  # the feed announces offers, and an existing row was already offered. The
  # single-writer GenServer serializes posts from this instance; posts from
  # sibling instances serialize on SQLite itself.
  def handle_call(
        {:post_request, kind, ref, title, body, session_id, at},
        _from,
        %{db: db} = state
      ) do
    :ok = execute!(db, "BEGIN IMMEDIATE")

    run(
      db,
      "INSERT OR IGNORE INTO requests (kind, ref, title, body, posted_by, status, posted_at) VALUES (?, ?, ?, ?, ?, 'open', ?)",
      [kind, ref, title, body, session_id, at]
    )

    {:ok, changes} = Sqlite3.changes(db.conn)

    id =
      if changes == 1 do
        {:ok, id} = Sqlite3.last_insert_rowid(db.conn)
        insert_request_event(db, id, "posted", session_id, at)
        id
      else
        [[id]] = fetch(db, "SELECT id FROM requests WHERE ref = ?", [ref])
        id
      end

    :ok = execute!(db, "COMMIT")
    {:reply, {:ok, fetch_request(db, id)}, state}
  end

  # The guarded UPDATE plus the changes count is the whole race (#3883): the
  # winner flips the one still-open row, every loser flips zero and reads
  # the standing row back. The single-writer GenServer serializes claims
  # from this instance; claims from sibling instances serialize on SQLite
  # itself (BEGIN IMMEDIATE takes the write lock before the UPDATE looks).
  def handle_call({:claim_request, id, session_id, at}, _from, %{db: db} = state) do
    :ok = execute!(db, "BEGIN IMMEDIATE")
    won = claim_request_row(db, id, session_id, at)
    :ok = execute!(db, "COMMIT")
    {:reply, claim_reply(db, id, session_id, won), state}
  end

  def handle_call({:finish_request, id, session_id, at}, _from, %{db: db} = state) do
    :ok = execute!(db, "BEGIN IMMEDIATE")

    run(
      db,
      "UPDATE requests SET status = 'done', done_at = ? WHERE id = ? AND status = 'claimed'",
      [at, id]
    )

    {:ok, changes} = Sqlite3.changes(db.conn)
    if changes == 1, do: insert_request_event(db, id, "done", session_id, at)
    :ok = execute!(db, "COMMIT")

    reply =
      case fetch_request(db, id) do
        nil ->
          {:error, :not_found}

        # Re-finishing a done request reads back as the finish it already
        # is (the client seam retries across a restart, #3903); a still-open
        # row was never claimed, and finishing unclaimed work stays an error.
        %{status: :done} = request ->
          {:ok, request}

        request ->
          {:error, request}
      end

    {:reply, reply, state}
  end

  # Issue pickup (#3880) as one transaction on the request bus: ensure the
  # kind='issue' row, then claim it. A row born here is claimed at birth and
  # writes only the claimed event -- it was never on offer, so a posted
  # announcement would offer work that is already taken.
  def handle_call({:claim_issue, repo, number, session_id, at}, _from, %{db: db} = state) do
    ref = "#{repo}##{number}"
    :ok = execute!(db, "BEGIN IMMEDIATE")

    run(
      db,
      "INSERT OR IGNORE INTO requests (kind, ref, title, posted_by, status, posted_at) VALUES ('issue', ?, ?, ?, 'open', ?)",
      [ref, ref, session_id, at]
    )

    [[id]] = fetch(db, "SELECT id FROM requests WHERE ref = ?", [ref])
    won = claim_request_row(db, id, session_id, at)
    :ok = execute!(db, "COMMIT")

    {:reply, claim_reply(db, id, session_id, won), state}
  end

  def handle_call(:list_requests, _from, %{db: db} = state) do
    rows =
      fetch(
        db,
        @select_request <>
          " ORDER BY CASE r.status WHEN 'open' THEN 0 WHEN 'claimed' THEN 1 ELSE 2 END, r.id DESC",
        []
      )

    {:reply, Enum.map(rows, &request_row_to_map/1), state}
  end

  def handle_call({:request_events_after, id}, _from, %{db: db} = state) do
    rows = fetch(db, @select_request_event <> " WHERE e.id > ? ORDER BY e.id", [id])
    {:reply, Enum.map(rows, &request_event_row_to_map/1), state}
  end

  def handle_call(:last_request_event_id, _from, %{db: db} = state) do
    [[id]] = fetch(db, "SELECT COALESCE(MAX(id), 0) FROM request_events", [])
    {:reply, id, state}
  end

  def handle_call({:mute_fleet_predicate, id, reason, at}, _from, %{db: db} = state) do
    run(
      db,
      "INSERT INTO fleet_mutes (predicate, muted_at, reason) VALUES (?, ?, ?) ON CONFLICT(predicate) DO NOTHING",
      [id, at, reason]
    )

    {:reply, :ok, state}
  end

  def handle_call({:unmute_fleet_predicate, id}, _from, %{db: db} = state) do
    run(db, "DELETE FROM fleet_mutes WHERE predicate = ?", [id])
    {:reply, :ok, state}
  end

  def handle_call(:fleet_mutes, _from, %{db: db} = state) do
    entries =
      for [predicate, muted_at, reason] <-
            fetch(db, "SELECT predicate, muted_at, reason FROM fleet_mutes ORDER BY muted_at", []) do
        %{id: predicate, muted_at: muted_at, reason: reason}
      end

    {:reply, entries, state}
  end

  # The INSERT is the decision, not a preceding SELECT: two kernels sharing
  # this file poll independently, and a check-then-insert would let both
  # announce the same fault.
  #
  # It must be DO NOTHING, and the refresh must be a separate UPDATE. With
  # `ON CONFLICT DO UPDATE` in one statement, `changes` is 1 for the update
  # branch as well as the insert, so every poll reads as new and a standing
  # fault re-announces forever -- which is the exact spam this dedup exists to
  # prevent. The break-test in fleet_alerts_test.exs caught it doing precisely
  # that; keep that test if you touch this.
  def handle_call(
        {:fleet_alert_seen, fingerprint, predicate, summary, at},
        _from,
        %{db: db} = state
      ) do
    run(
      db,
      "INSERT INTO fleet_alerts_seen (fingerprint, predicate, summary, first_seen_at, last_seen_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(fingerprint) DO NOTHING",
      [fingerprint, predicate, summary, at, at]
    )

    {:ok, inserted} = Sqlite3.changes(db.conn)

    # Refresh regardless of who won, so "still true" stays distinguishable
    # from "gone" when reading fleet_alerts_seen.
    run(db, "UPDATE fleet_alerts_seen SET last_seen_at = ? WHERE fingerprint = ?", [
      at,
      fingerprint
    ])

    {:reply, inserted == 1, state}
  end

  def handle_call(:fleet_alerts_seen, _from, %{db: db} = state) do
    entries =
      for [fingerprint, predicate, summary, first_seen_at, last_seen_at] <-
            fetch(
              db,
              "SELECT fingerprint, predicate, summary, first_seen_at, last_seen_at FROM fleet_alerts_seen ORDER BY last_seen_at DESC",
              []
            ) do
        %{
          fingerprint: fingerprint,
          predicate: predicate,
          summary: summary,
          first_seen_at: first_seen_at,
          last_seen_at: last_seen_at
        }
      end

    {:reply, entries, state}
  end

  def handle_call({:forget_fleet_alerts, :all}, _from, %{db: db} = state) do
    run(db, "DELETE FROM fleet_alerts_seen", [])
    {:ok, changes} = Sqlite3.changes(db.conn)
    {:reply, changes, state}
  end

  def handle_call({:forget_fleet_alerts, predicate}, _from, %{db: db} = state)
      when is_binary(predicate) do
    run(db, "DELETE FROM fleet_alerts_seen WHERE predicate = ?", [predicate])
    {:ok, changes} = Sqlite3.changes(db.conn)
    {:reply, changes, state}
  end

  def handle_call({:heartbeat_session, session_id, at}, _from, %{db: db} = state) do
    run(db, "UPDATE sessions SET last_seen_at = ? WHERE id = ?", [at, session_id])
    {:reply, :ok, state}
  end

  def handle_call(:session_directory, _from, %{db: db} = state) do
    entries =
      for [id, name, topic, started_at, last_seen_at] <- fetch(db, @select_directory, []) do
        %{id: id, name: name, topic: topic, started_at: started_at, last_seen_at: last_seen_at}
      end

    {:reply, entries, state}
  end

  def handle_call({:send_session_message, from, to, body, at}, _from, %{db: db} = state) do
    run(
      db,
      "INSERT INTO session_messages (from_session, to_session, body, created_at) VALUES (?, ?, ?, ?)",
      [from, to, body, at]
    )

    {:ok, id} = Sqlite3.last_insert_rowid(db.conn)
    [row] = fetch(db, @select_session_message <> " WHERE m.id = ?", [id])
    {:reply, {:ok, session_message_row_to_map(row)}, state}
  end

  def handle_call({:session_messages_after, id, session_id}, _from, %{db: db} = state) do
    rows =
      fetch(
        db,
        @select_session_message <>
          " WHERE m.id > ? AND m.from_session != ? AND (m.to_session IS NULL OR m.to_session = ?) ORDER BY m.id",
        [id, session_id, session_id]
      )

    {:reply, Enum.map(rows, &session_message_row_to_map/1), state}
  end

  def handle_call(:last_session_message_id, _from, %{db: db} = state) do
    [[id]] = fetch(db, "SELECT COALESCE(MAX(id), 0) FROM session_messages", [])
    {:reply, id, state}
  end

  def handle_call({:unacked_outbox, session_id}, _from, %{db: db} = state) do
    {sql, params} =
      case session_id do
        nil ->
          {"SELECT o.id, o.job_id, o.intent, o.status, o.elapsed_ms, o.result, j.session_id " <>
             "FROM outbox o LEFT JOIN jobs j ON j.id = o.job_id WHERE o.acked = 0 ORDER BY o.id",
           []}

        sid ->
          {"SELECT o.id, o.job_id, o.intent, o.status, o.elapsed_ms, o.result, j.session_id " <>
             "FROM outbox o JOIN jobs j ON j.id = o.job_id " <>
             "WHERE j.session_id IS ? AND o.acked = 0 ORDER BY o.id", [sid]}
      end

    {:reply, Enum.map(fetch(db, sql, params), &outbox_row_to_map/1), state}
  end

  # The reply-time suppression flip (#3934): one UPDATE, no read-back --
  # a job has at most one outbox row in the normal path, and a ledger-retry
  # duplicate deserves the same silence.
  def handle_call({:ack_job_outbox, job_id}, _from, %{db: db} = state) do
    run(db, "UPDATE outbox SET acked = 1 WHERE job_id = ? AND acked = 0", [job_id])
    {:ok, changes} = Sqlite3.changes(db.conn)
    {:reply, changes, state}
  end

  # Claim each row that is still unacked; the count of rows this call flips is
  # the arbiter that keeps a racing publish and replay from double-announcing
  # (#3839). The SELECT-then-UPDATE is atomic here because it runs inside one
  # call on the single-writer GenServer.
  def handle_call({:ack_outbox, ids}, _from, %{db: db} = state) do
    claimed =
      Enum.reduce(ids, 0, fn id, acc ->
        case fetch(db, "SELECT acked FROM outbox WHERE id = ?", [id]) do
          [[0]] ->
            run(db, "UPDATE outbox SET acked = 1 WHERE id = ?", [id])
            acc + 1

          _ ->
            acc
        end
      end)

    {:reply, claimed, state}
  end

  defp ensure_version(db) do
    case user_version(db) do
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
        case unstamped_version(db) do
          :fresh -> create(db)
          version -> migrate(db, version)
        end

      found ->
        migrate(db, found)
    end
  end

  defp user_version(db) do
    [[version]] = fetch(db, "PRAGMA user_version", [])
    version
  end

  defp unstamped_version(db) do
    case table_columns(db, "actions") do
      [] -> :fresh
      columns -> sniff_version(db, columns)
    end
  end

  # Ordered shape probes, oldest first: the first missing piece names the
  # version the file stopped at. Column probes here, table probes in
  # sniff_table_version/1 -- split because the two ask different questions of
  # different catalogs, and together they exceed the complexity budget.
  defp sniff_version(db, columns) do
    cond do
      "session_id" not in columns -> 1
      "status" not in columns -> 2
      "line" not in columns -> 3
      true -> sniff_table_version(db)
    end
  end

  # Newest table first. v8 dropped issue_claims (#3883), so its absence cannot
  # distinguish v5 from v8 -- only the presence of a table introduced later
  # can, and the same reasoning makes fleet_mutes (ENG-11209) the test for v9.
  # Reading these in the wrong order silently mis-dates the file and runs a
  # migration ladder from the wrong rung.
  defp sniff_table_version(db) do
    cond do
      table_exists?(db, "fleet_mutes") -> @user_version
      table_exists?(db, "requests") -> 8
      not table_exists?(db, "jobs") -> 4
      not table_exists?(db, "issue_claims") -> 5
      not table_exists?(db, "session_messages") -> 6
      true -> 7
    end
  end

  defp table_exists?(db, table) do
    fetch(db, "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?", [table]) != []
  end

  defp create(db) do
    execute_all(
      db,
      [
        "BEGIN IMMEDIATE",
        @create_sessions,
        @create_topics,
        @create_actions,
        @create_jobs,
        @create_job_output,
        @create_outbox,
        @create_requests,
        @create_request_events,
        @create_session_messages,
        @create_fleet_mutes,
        @create_fleet_alerts_seen,
        stamp(),
        "COMMIT"
      ]
    )
  end

  # An already-current file from before stamping existed: mark it, move on.
  defp migrate(db, @user_version), do: execute_all(db, [stamp()])

  defp migrate(db, from) do
    @migrations
    |> Enum.drop_while(fn {version, _statements} -> version < from end)
    |> Enum.each(fn {version, statements} ->
      execute_all(db, ["BEGIN IMMEDIATE"] ++ statements ++ [stamp(version + 1), "COMMIT"])
    end)
  end

  defp stamp(version \\ @user_version), do: "PRAGMA user_version = #{version}"

  defp disabled_reply({:create_session, _name, _at}), do: 0
  defp disabled_reply({:create_topic, _session_id, _name, _at}), do: 0
  defp disabled_reply({:start_action, _action}), do: 0

  defp disabled_reply({:finish_job, _id, _status, _result, _quiet, _at, _start}),
    do: :already_final

  defp disabled_reply({:post_request, _kind, _ref, _title, _body, _session_id, _at}),
    do: :disabled

  defp disabled_reply({:claim_request, _id, _session_id, _at}), do: :disabled
  defp disabled_reply({:finish_request, _id, _session_id, _at}), do: :disabled
  defp disabled_reply({:claim_issue, _repo, _number, _session_id, _at}), do: :disabled
  defp disabled_reply(:list_requests), do: []
  defp disabled_reply({:request_events_after, _id}), do: []
  defp disabled_reply(:last_request_event_id), do: 0
  # A mute that was not stored must not report success. Answering :ok here
  # would tell an operator their unsubscribe took effect while the next poll
  # announces the same thing again -- the precise experience that teaches
  # people muting does not work. :disabled is the same answer the request bus
  # gives for the same reason.
  defp disabled_reply({:mute_fleet_predicate, _id, _reason, _at}), do: :disabled
  defp disabled_reply({:unmute_fleet_predicate, _id}), do: :disabled
  defp disabled_reply(:fleet_mutes), do: []
  # No ledger means no dedup record, so every poll would re-announce. Say
  # "not new" instead: a degraded log must not turn one fault into a stream.
  defp disabled_reply({:fleet_alert_seen, _fp, _pred, _summary, _at}), do: false
  defp disabled_reply(:fleet_alerts_seen), do: []
  defp disabled_reply({:forget_fleet_alerts, _scope}), do: 0
  defp disabled_reply({:heartbeat_session, _session_id, _at}), do: :ok
  defp disabled_reply(:session_directory), do: []
  defp disabled_reply({:send_session_message, _from, _to, _body, _at}), do: :disabled
  defp disabled_reply({:session_messages_after, _id, _session_id}), do: []
  defp disabled_reply(:last_session_message_id), do: 0
  defp disabled_reply({:job, _id}), do: nil
  defp disabled_reply({:job_output, _id}), do: ""
  defp disabled_reply({:recent_jobs, _session_id, _n}), do: []
  defp disabled_reply({:unacked_outbox, _session_id}), do: []
  defp disabled_reply({:ack_outbox, _ids}), do: 0
  defp disabled_reply({:ack_job_outbox, _job_id}), do: 0
  defp disabled_reply({:recent, _n}), do: []
  defp disabled_reply(:sessions), do: []
  defp disabled_reply(:topics), do: []
  defp disabled_reply(_request), do: :ok

  defp table_columns(db, table) do
    for [_cid, name | _rest] <- fetch(db, "PRAGMA table_info(#{table})", []), do: name
  end

  defp execute_all(db, statements) do
    Enum.each(statements, fn statement -> :ok = execute!(db, statement) end)
  end

  defp run(db, sql, params) do
    {:ok, statement} = Sqlite3.prepare(db.conn, sql)
    :ok = Sqlite3.bind(statement, params)
    :done = step!(db, statement, sql)
    :ok = Sqlite3.release(db.conn, statement)
  end

  # :busy is a legitimate runtime state -- a sibling instance still holds
  # the write lock -- not a programming error. Each attempt waits only the
  # short NIF-level bound; the full busy budget is a wall-clock deadline
  # ridden out in scheduler-free sleeps so the sibling's release can always
  # get a dirty IO scheduler (#3903). A lock outliving the budget fails
  # with a diagnosis instead of a bare badmatch (#3890), and the supervisor
  # reopens the log after the crash. Re-stepping after :busy is sqlite's
  # documented retry protocol for SQLITE_BUSY.
  defp step!(db, statement, sql) do
    busy_wait(db, sql, "write", fn ->
      case Sqlite3.step(db.conn, statement) do
        :busy -> :busy
        result -> {:done, result}
      end
    end)
  end

  defp fetch(db, sql, params) do
    {:ok, statement} = Sqlite3.prepare(db.conn, sql)
    :ok = Sqlite3.bind(statement, params)

    busy_wait(db, sql, "read", fn ->
      case Sqlite3.fetch_all(db.conn, statement) do
        {:ok, rows} ->
          :ok = Sqlite3.release(db.conn, statement)
          {:done, rows}

        # fetch_all steps to completion, so a busy result leaves the
        # statement mid-walk; reset rewinds it for the next attempt.
        {:error, "Database busy"} ->
          :ok = Sqlite3.reset(statement)
          :busy

        {:error, reason} ->
          raise "action log read failed: #{inspect(reason)}: #{sql}"
      end
    end)
  end

  # The transaction-control sibling of step!/fetch (#3874): the job-ledger
  # writes wrap several statements in BEGIN IMMEDIATE...COMMIT through
  # Sqlite3.execute/2, whose busy result surfaced as
  # `{:badmatch, {:error, "database is locked"}}` in the incident. Same
  # deadline policy as step!; a residual lock fails with a diagnosis and
  # the supervisor reopens the log.
  defp execute!(db, sql) do
    busy_wait(db, sql, "write", fn ->
      case Sqlite3.execute(db.conn, sql) do
        :ok -> {:done, :ok}
        {:error, "database is locked"} -> :busy
        {:error, reason} -> raise "action log execute failed: #{inspect(reason)}: #{sql}"
      end
    end)
  end

  defp busy_wait(db, sql, verb, attempt) do
    deadline = System.monotonic_time(:millisecond) + db.busy_timeout_ms
    busy_wait(db, sql, verb, attempt, deadline)
  end

  defp busy_wait(db, sql, verb, attempt, deadline) do
    case attempt.() do
      {:done, result} ->
        result

      :busy ->
        if System.monotonic_time(:millisecond) < deadline do
          Process.sleep(@busy_poll_ms)
          busy_wait(db, sql, verb, attempt, deadline)
        else
          raise "action log #{verb} still blocked after the busy-timeout wait (#3890): #{sql}"
        end
    end
  end

  defp busy_budget(opts), do: Keyword.get(opts, :busy_timeout_ms, @busy_timeout_ms)

  defp state_home do
    System.get_env("XDG_STATE_HOME") || Path.join(System.user_home!(), ".local/state")
  end

  defp now, do: DateTime.utc_now() |> DateTime.to_iso8601()

  # One idempotent write shared by `job_started` and the reconstructing
  # `finish_job` (#4082): INSERT OR IGNORE on the primary key, so replays
  # and reconstructions never clobber a row that already landed.
  defp insert_job_row(db, job) do
    run(
      db,
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
  end

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

  defp session_message_row_to_map([id, from_session, from, to_session, body, created_at]) do
    %{
      id: id,
      from_session: from_session,
      from: from,
      to_session: to_session,
      body: body,
      created_at: created_at
    }
  end

  # The claim step shared by claim_request and claim_issue (#3883): the
  # guarded UPDATE, its row count, and the claimed event, inside the
  # caller's open transaction. True means this call flipped the row.
  defp claim_request_row(db, id, session_id, at) do
    run(
      db,
      "UPDATE requests SET status = 'claimed', claimed_by = ?, claimed_at = ? WHERE id = ? AND status = 'open'",
      [session_id, at, id]
    )

    {:ok, changes} = Sqlite3.changes(db.conn)
    if changes == 1, do: insert_request_event(db, id, "claimed", session_id, at)
    changes == 1
  end

  # A standing claim by the caller's own session is a win, not a loss: the
  # client seam retries a claim whose server died after committing (#3903),
  # and that retry must read back as the victory it already is, or the sole
  # claimant would report losing the request to itself. Only a real session
  # id gets this: nil belongs to every anonymous caller at once, so it can
  # never prove the standing claim is the caller's own.
  defp claim_reply(db, id, session_id, won) do
    case fetch_request(db, id) do
      nil ->
        {:error, :not_found}

      request ->
        if won or
             (session_id != nil and request.status == :claimed and
                request.claimed_by == session_id) do
          {:ok, request}
        else
          {:error, request}
        end
    end
  end

  defp insert_request_event(db, request_id, event, session_id, at) do
    run(
      db,
      "INSERT INTO request_events (request_id, event, session_id, at) VALUES (?, ?, ?, ?)",
      [request_id, event, session_id, at]
    )
  end

  defp fetch_request(db, id) do
    case fetch(db, @select_request <> " WHERE r.id = ?", [id]) do
      [] -> nil
      [row] -> request_row_to_map(row)
    end
  end

  defp request_kind_atom("issue"), do: :issue
  defp request_kind_atom("adhoc"), do: :adhoc

  defp request_status_atom("open"), do: :open
  defp request_status_atom("claimed"), do: :claimed
  defp request_status_atom("done"), do: :done

  defp request_event_atom("posted"), do: :posted
  defp request_event_atom("claimed"), do: :claimed
  defp request_event_atom("done"), do: :done

  defp request_row_to_map([
         id,
         kind,
         ref,
         title,
         body,
         posted_by,
         poster,
         status,
         claimed_by,
         claimer,
         posted_at,
         claimed_at,
         done_at
       ]) do
    %{
      id: id,
      kind: request_kind_atom(kind),
      ref: ref,
      title: title,
      body: body,
      posted_by: posted_by,
      poster: poster,
      status: request_status_atom(status),
      claimed_by: claimed_by,
      claimer: claimer,
      posted_at: posted_at,
      claimed_at: claimed_at,
      done_at: done_at
    }
  end

  defp request_event_row_to_map([
         id,
         request_id,
         event,
         session_id,
         session,
         at,
         kind,
         ref,
         title,
         body
       ]) do
    %{
      id: id,
      request_id: request_id,
      event: request_event_atom(event),
      session_id: session_id,
      session: session,
      at: at,
      kind: request_kind_atom(kind),
      ref: ref,
      title: title,
      body: body
    }
  end

  defp outbox_row_to_map([id, job_id, intent, status, elapsed_ms, result, session_id]) do
    %{
      id: id,
      job_id: job_id,
      intent: intent,
      status: status_atom(status),
      elapsed_ms: elapsed_ms,
      result: result,
      session_id: session_id,
      # Only unacked rows are ever read back into maps.
      acked: false
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
