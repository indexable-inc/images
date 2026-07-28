defmodule IxMcp.ActionLogTest do
  use ExUnit.Case, async: false

  import ExUnit.CaptureLog

  alias Exqlite.Sqlite3
  alias IxMcp.ActionLog
  alias IxMcp.MCP.Server

  defp tmp_db do
    path =
      Path.join(
        System.tmp_dir!(),
        "ix-mcp-action-log-test-#{System.unique_integer([:positive])}.db"
      )

    on_exit(fn -> File.rm(path) end)
    path
  end

  defp user_version(path) do
    {:ok, conn} = Sqlite3.open(path)
    {:ok, statement} = Sqlite3.prepare(conn, "PRAGMA user_version")
    {:ok, [[version]]} = Sqlite3.fetch_all(conn, statement)
    :ok = Sqlite3.release(conn, statement)
    :ok = Sqlite3.close(conn)
    version
  end

  test "a tools/call lands one row under the current session and topic" do
    :ok = IxMcp.Session.set_name("action-log-test")
    :ok = IxMcp.Session.set_topic("logging")

    response =
      Server.handle(%{
        "jsonrpc" => "2.0",
        "id" => 1,
        "method" => "tools/call",
        "params" => %{
          "name" => "exec",
          "arguments" => %{"code" => "1 + 1", "intent" => "log probe"}
        }
      })

    assert %{"result" => %{"isError" => false}} = response

    assert [entry | _rest] = ActionLog.recent(1)
    assert entry.tool == "exec"
    assert entry.session == "action-log-test"
    assert entry.topic == "logging"
    assert entry.intent == "log probe"
    refute entry.is_error
    assert entry.status == "done"
    assert entry.stack == nil
    assert entry.elapsed_ms >= 0
    assert {:ok, %DateTime{}, 0} = DateTime.from_iso8601(entry.at)
  end

  test "a failing tool call is recorded with is_error and its intent" do
    response =
      Server.handle(%{
        "jsonrpc" => "2.0",
        "id" => 2,
        "method" => "tools/call",
        "params" => %{
          "name" => "exec",
          "arguments" => %{"intent" => "not really code", "budget" => "bogus"}
        }
      })

    assert %{"result" => %{"isError" => true}} = response

    assert [entry | _rest] = ActionLog.recent(1)
    assert entry.tool == "exec"
    assert entry.intent == "not really code"
    assert entry.is_error
    assert entry.status == "failed"
  end

  test "an exec row is running before the eval finishes, then finalizes with its true elapsed" do
    response =
      Server.handle(%{
        "jsonrpc" => "2.0",
        "id" => 5,
        "method" => "tools/call",
        "params" => %{
          "name" => "exec",
          "arguments" => %{
            "code" => "Process.sleep(250); :ok",
            "intent" => "outlive the budget",
            "budget" => 0.05
          }
        }
      })

    [%{"text" => text}] = response["result"]["content"]
    %{"job" => job_id, "running" => true} = text |> String.split("\n") |> hd() |> JSON.decode!()

    # The wire response shipped while the eval still runs: the row says so.
    assert [%{tool: "exec", status: "running", elapsed_ms: 0} | _] = ActionLog.recent(1)

    assert %{status: :done} = IxMcp.Jobs.await(job_id, 5_000)

    # finish_action runs before the awaiting caller wakes, so the row is
    # already final here -- with the eval's elapsed, not the wire budget's.
    assert [entry | _] = ActionLog.recent(1)
    assert entry.status == "done"
    refute entry.is_error
    assert entry.elapsed_ms >= 200
    assert entry.stack == nil
  end

  test "a running exec's current_stacktrace is sampled into the row; cancel finalizes it" do
    response =
      Server.handle(%{
        "jsonrpc" => "2.0",
        "id" => 6,
        "method" => "tools/call",
        "params" => %{
          "name" => "exec",
          "arguments" => %{
            "code" => "Process.sleep(:infinity)",
            "intent" => "wedge for sampling",
            "budget" => 0.05
          }
        }
      })

    [%{"text" => text}] = response["result"]["content"]
    %{"job" => job_id} = text |> String.split("\n") |> hd() |> JSON.decode!()

    # The sampler ticks every 25ms under test config; give it a few ticks.
    entry =
      poll_until(fn ->
        case ActionLog.recent(1) do
          [%{status: "running", stack: stack} = entry | _] when is_binary(stack) -> entry
          _ -> nil
        end
      end)

    frames = JSON.decode!(entry.stack)
    assert Enum.any?(frames, &(&1 =~ "sleep")), "expected a sleep frame in #{inspect(frames)}"

    # Machinery below the eval boundary is pruned from the sample, so the
    # innermost stored frame is one the cell author can act on (#3546).
    refute Enum.any?(frames, &(&1 =~ "erl_eval")), "expected pruned frames in #{inspect(frames)}"

    # A top-level statement runs interpreted: no frame is the cell's own, so
    # there is no cell line and a viewer falls back to the top frame.
    assert entry.line == nil

    :ok = IxMcp.Jobs.cancel(job_id)

    assert [entry | _] = ActionLog.recent(1)
    assert entry.status == "cancelled"
    refute entry.is_error
    assert entry.stack == nil, "finalizing clears the sampled stack"
    assert entry.line == nil, "finalizing clears the sampled cell line"
  end

  test "a cell-owned frame's line is sampled into the row as the live cell line" do
    # Code compiled in the cell (the module it defines) carries
    # `file: "cell"` in its frames; line 3 is the Process.sleep call site.
    code = """
    defmodule IxMcp.ActionLogTest.CellLineProbe do
      def loop do
        Process.sleep(50)
        loop()
      end
    end

    IxMcp.ActionLogTest.CellLineProbe.loop()
    """

    response =
      Server.handle(%{
        "jsonrpc" => "2.0",
        "id" => 7,
        "method" => "tools/call",
        "params" => %{
          "name" => "exec",
          "arguments" => %{"code" => code, "intent" => "loop for line sampling", "budget" => 0.05}
        }
      })

    [%{"text" => text}] = response["result"]["content"]
    %{"job" => job_id} = text |> String.split("\n") |> hd() |> JSON.decode!()

    entry =
      poll_until(fn ->
        case ActionLog.recent(1) do
          [%{status: "running", line: line} = entry | _] when is_integer(line) -> entry
          _ -> nil
        end
      end)

    assert entry.line == 3

    # The stored frames name the cell line too, right under the sleep frame.
    frames = JSON.decode!(entry.stack)
    assert Enum.any?(frames, &(&1 =~ "cell:3")), "expected a cell:3 frame in #{inspect(frames)}"

    :ok = IxMcp.Jobs.cancel(job_id)
    assert [%{status: "cancelled", line: nil} | _] = ActionLog.recent(1)
  end

  defp poll_until(fun, deadline_ms \\ 2_000) do
    deadline = System.monotonic_time(:millisecond) + deadline_ms
    do_poll(fun, deadline)
  end

  defp do_poll(fun, deadline) do
    case fun.() do
      nil ->
        if System.monotonic_time(:millisecond) >= deadline do
          flunk("condition not met within #{deadline}ms deadline")
        else
          Process.sleep(10)
          do_poll(fun, deadline)
        end

      value ->
        value
    end
  end

  test "one server instance is one session; a topic is a timeline, not a dictionary" do
    # Prime the lazy session row so the count below is stable regardless of
    # which test in this suite touched the global log first.
    %{session_id: session_id} = IxMcp.Session.ids()
    before_sessions = ActionLog.sessions()

    :ok = IxMcp.Session.set_topic("repeated")
    :ok = IxMcp.Session.set_topic("repeated")

    # Repeating a topic name makes a new row each time...
    repeated = Enum.filter(ActionLog.topics(), &(&1.name == "repeated"))
    assert [%{id: first_id}, %{id: second_id}] = repeated
    assert second_id > first_id

    # ...while the session row count never grows within one instance.
    assert length(ActionLog.sessions()) == length(before_sessions)

    # Every topic hangs off this instance's single session row.
    assert %{session_id: ^session_id} = IxMcp.Session.ids()
    assert Enum.all?(repeated, &(&1.session_id == session_id))
  end

  test "a log with no recorded actions has no session rows" do
    log = start_supervised!({ActionLog, path: tmp_db(), name: :action_log_lazy_test})
    assert ActionLog.sessions(log) == []
  end

  test "rows persist in the file across a log restart" do
    path = tmp_db()

    log = start_supervised!({ActionLog, path: path, name: :action_log_file_test}, id: :first_open)

    session_id = ActionLog.create_session("s", log)
    topic_id = ActionLog.create_topic(session_id, "t", log)

    action_id =
      ActionLog.start_action(
        %{
          session_id: session_id,
          topic_id: topic_id,
          tool: "exec",
          intent: "persist me",
          arguments: "{}"
        },
        log
      )

    assert [%{intent: "persist me", status: "running"}] = ActionLog.recent(20, log)
    :ok = ActionLog.finish_action(action_id, "done", false, 7, log)

    assert [%{intent: "persist me", status: "done", elapsed_ms: 7}] = ActionLog.recent(20, log)
    stop_supervised!(:first_open)

    reopened =
      start_supervised!({ActionLog, path: path, name: :action_log_reopen_test}, id: :reopen)

    assert [%{intent: "persist me", tool: "exec", session: "s", topic: "t"}] =
             ActionLog.recent(20, reopened)
  end

  test "a pre-live v2 database gains the status/stack columns; its rows read as done" do
    path = tmp_db()

    # The #3532 v2 shape, before the live-row columns (#3536).
    {:ok, conn} = Sqlite3.open(path)

    for statement <- [
          "CREATE TABLE sessions (id INTEGER PRIMARY KEY, name TEXT, started_at TEXT NOT NULL)",
          "CREATE TABLE topics (id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id), name TEXT NOT NULL, started_at TEXT NOT NULL)",
          "CREATE TABLE actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id), topic_id INTEGER REFERENCES topics(id), tool TEXT NOT NULL, intent TEXT, arguments TEXT NOT NULL, is_error INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL)",
          "INSERT INTO sessions (id, name, started_at) VALUES (1, 'old', '2026-07-17T10:00:00Z')",
          "INSERT INTO actions (at, session_id, topic_id, tool, intent, arguments, is_error, elapsed_ms) VALUES ('2026-07-17T10:00:01Z', 1, NULL, 'exec', 'pre-live row', '{}', 0, 5)"
        ] do
      :ok = Sqlite3.execute(conn, statement)
    end

    :ok = Sqlite3.close(conn)

    log = start_supervised!({ActionLog, path: path, name: :action_log_pre_live})

    # Old rows were written after the fact, so 'done' is their true status.
    assert [%{intent: "pre-live row", status: "done", stack: nil}] = ActionLog.recent(10, log)

    # The migrated table supports the live lifecycle end to end.
    action_id =
      ActionLog.start_action(
        %{session_id: 1, topic_id: nil, tool: "exec", intent: "new row", arguments: "{}"},
        log
      )

    :ok = ActionLog.update_stack(action_id, ~s(["frame"]), 2, log)
    assert [%{status: "running", stack: ~s(["frame"]), line: 2} | _] = ActionLog.recent(10, log)
    :ok = ActionLog.finish_action(action_id, "done", false, 3, log)
    assert [%{status: "done", stack: nil, line: nil} | _] = ActionLog.recent(10, log)

    # The 2 -> ... -> 8 steps stamped the file on their way through (index#3539).
    assert user_version(path) == 8
  end

  test "a v1 database migrates losslessly to the normalized schema, once" do
    path = tmp_db()

    # Build the pre-#3532 shape by hand: session/topic as TEXT per row.
    {:ok, conn} = Sqlite3.open(path)

    :ok =
      Sqlite3.execute(conn, """
      CREATE TABLE actions (
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
      """)

    rows = [
      # unnamed session, no topic
      ["2026-01-01T00:00:00Z", nil, nil, "elixir_exec", "a", "{}", 0, 1],
      # unnamed session, topic: NULL sessions collapse into one
      ["2026-01-01T00:01:00Z", nil, "loose", "read", nil, "{}", 0, 2],
      # named session, no topic yet
      ["2026-01-02T00:00:00Z", "alpha", nil, "elixir_exec", "b", "{}", 1, 3],
      # named session, repeated (session, topic) pair maps to one topic row
      ["2026-01-02T00:01:00Z", "alpha", "build", "elixir_exec", "c", "{}", 0, 4],
      ["2026-01-02T00:02:00Z", "alpha", "build", "kernel_trace", nil, "{}", 0, 5],
      # same topic name under another session is a distinct topic row
      ["2026-01-03T00:00:00Z", "beta", "build", "elixir_exec", "d", "{}", 0, 6]
    ]

    for row <- rows do
      {:ok, statement} =
        Sqlite3.prepare(
          conn,
          "INSERT INTO actions (at, session, topic, tool, intent, arguments, is_error, elapsed_ms) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )

      :ok = Sqlite3.bind(statement, row)
      :done = Sqlite3.step(conn, statement)
      :ok = Sqlite3.release(conn, statement)
    end

    :ok = Sqlite3.close(conn)

    # Opening the log migrates in place.
    log = start_supervised!({ActionLog, path: path, name: :action_log_migration}, id: :migrate)

    sessions = ActionLog.sessions(log)
    assert sessions |> Enum.map(& &1.name) |> Enum.sort() == Enum.sort([nil, "alpha", "beta"])

    unnamed = Enum.find(sessions, &is_nil(&1.name))
    alpha = Enum.find(sessions, &(&1.name == "alpha"))
    beta = Enum.find(sessions, &(&1.name == "beta"))
    # started_at backfills from each session's earliest action.
    assert alpha.started_at == "2026-01-02T00:00:00Z"

    topics = ActionLog.topics(log)

    assert topics |> Enum.map(&{&1.session_id, &1.name}) |> Enum.sort() ==
             Enum.sort([{unnamed.id, "loose"}, {alpha.id, "build"}, {beta.id, "build"}])

    # Every v1 action survives, remapped onto ids (recent/2 is newest first).
    entries = ActionLog.recent(10, log)

    assert Enum.map(entries, &{&1.session, &1.topic, &1.tool, &1.elapsed_ms}) == [
             {"beta", "build", "elixir_exec", 6},
             {"alpha", "build", "kernel_trace", 5},
             {"alpha", "build", "elixir_exec", 4},
             {"alpha", nil, "elixir_exec", 3},
             {nil, "loose", "read", 2},
             {nil, nil, "elixir_exec", 1}
           ]

    stop_supervised!(:migrate)

    # The ladder ran 1 -> 2 -> ... -> 8 and left the stamp behind (index#3539).
    assert user_version(path) == 8

    # The migrated file is the v2 shape on disk: normalized columns, no v1
    # leftovers, and a reopen (no-op detection) does not duplicate rows.
    {:ok, conn} = Sqlite3.open(path)
    {:ok, statement} = Sqlite3.prepare(conn, "PRAGMA table_info(actions)")
    {:ok, info} = Sqlite3.fetch_all(conn, statement)
    :ok = Sqlite3.release(conn, statement)
    columns = for [_cid, name | _rest] <- info, do: name

    assert columns == [
             "id",
             "at",
             "session_id",
             "topic_id",
             "tool",
             "intent",
             "arguments",
             "is_error",
             "elapsed_ms",
             "status",
             "stack",
             "stack_at",
             "line"
           ]

    {:ok, statement} =
      Sqlite3.prepare(
        conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name"
      )

    {:ok, tables} = Sqlite3.fetch_all(conn, statement)
    :ok = Sqlite3.release(conn, statement)

    assert tables ==
             [
               ["actions"],
               ["job_output"],
               ["jobs"],
               ["outbox"],
               ["request_events"],
               ["requests"],
               ["session_messages"],
               ["sessions"],
               ["topics"]
             ]

    :ok = Sqlite3.close(conn)

    reopened = start_supervised!({ActionLog, path: path, name: :action_log_remigration})
    reopened_sessions = ActionLog.sessions(reopened)

    assert reopened_sessions |> Enum.map(& &1.name) |> Enum.sort() ==
             Enum.sort([nil, "alpha", "beta"])

    assert ActionLog.recent(10, reopened) |> Enum.map(&{&1.session, &1.tool}) == [
             {"beta", "elixir_exec"},
             {"alpha", "kernel_trace"},
             {"alpha", "elixir_exec"},
             {"alpha", "elixir_exec"},
             {nil, "read"},
             {nil, "elixir_exec"}
           ]
  end

  test "a fresh database is created stamped with the current schema version" do
    path = tmp_db()
    start_supervised!({ActionLog, path: path, name: :action_log_fresh_stamp})
    assert user_version(path) == 8
  end

  test "an unstamped file already at the current schema is stamped, not rewritten" do
    path = tmp_db()

    log = start_supervised!({ActionLog, path: path, name: :action_log_stamp_a}, id: :stamp_first)
    session_id = ActionLog.create_session("s", log)

    _action_id =
      ActionLog.start_action(
        %{session_id: session_id, topic_id: nil, tool: "exec", intent: "keep", arguments: "{}"},
        log
      )

    stop_supervised!(:stamp_first)

    # Rewind the stamp: this is exactly what a current-schema file written by
    # a pre-user_version server (any #3536-era binary) looks like on disk.
    {:ok, conn} = Sqlite3.open(path)
    :ok = Sqlite3.execute(conn, "PRAGMA user_version = 0")
    :ok = Sqlite3.close(conn)

    reopened = start_supervised!({ActionLog, path: path, name: :action_log_stamp_b})
    assert [%{intent: "keep"}] = ActionLog.recent(10, reopened)
    assert user_version(path) == 8
  end

  test "an unstamped pre-line file (the #3536 shape) sniffs as v3 and gains the line column" do
    path = tmp_db()

    # The v3 shape exactly as pre-stamping #3536-era binaries wrote it:
    # status/stack columns present, line absent, user_version still 0.
    {:ok, conn} = Sqlite3.open(path)

    for statement <- [
          "CREATE TABLE sessions (id INTEGER PRIMARY KEY, name TEXT, started_at TEXT NOT NULL)",
          "CREATE TABLE topics (id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id), name TEXT NOT NULL, started_at TEXT NOT NULL)",
          "CREATE TABLE actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id), topic_id INTEGER REFERENCES topics(id), tool TEXT NOT NULL, intent TEXT, arguments TEXT NOT NULL, is_error INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'done', stack TEXT, stack_at TEXT)",
          "INSERT INTO sessions (id, name, started_at) VALUES (1, 'old', '2026-07-17T10:00:00Z')",
          "INSERT INTO actions (at, session_id, topic_id, tool, intent, arguments, is_error, elapsed_ms) VALUES ('2026-07-17T10:00:01Z', 1, NULL, 'exec', 'pre-line row', '{}', 0, 5)"
        ] do
      :ok = Sqlite3.execute(conn, statement)
    end

    :ok = Sqlite3.close(conn)

    log = start_supervised!({ActionLog, path: path, name: :action_log_pre_line})

    assert [%{intent: "pre-line row", status: "done", line: nil}] = ActionLog.recent(10, log)
    assert user_version(path) == 8
  end

  test "the guarded update arbitrates issue claims on the request bus (#3880, #3883)" do
    log = start_supervised!({ActionLog, path: ":memory:", name: :action_log_claims})

    winner = ActionLog.create_session("winner", log)
    loser = ActionLog.create_session(nil, log)

    assert ActionLog.last_request_event_id(log) == 0

    assert {:ok, claim} = ActionLog.claim_issue("indexable-inc/index", 3880, winner, log)

    assert %{
             kind: :issue,
             ref: "indexable-inc/index#3880",
             status: :claimed,
             claimer: "winner"
           } = claim

    assert {:ok, %DateTime{}, 0} = DateTime.from_iso8601(claim.claimed_at)

    # The holder re-claiming its own issue reads back as the win it already
    # is: the client seam may retry a claim whose first attempt committed
    # just before the server died, and that retry must not report the sole
    # claimant as losing to itself (#3903).
    assert {:ok, ^claim} = ActionLog.claim_issue("indexable-inc/index", 3880, winner, log)

    # The loser reads the standing claim back, name included.
    assert {:error, standing} = ActionLog.claim_issue("indexable-inc/index", 3880, loser, log)
    assert standing.id == claim.id
    assert standing.claimer == "winner"

    # Same number on another repo is a different claim.
    assert {:ok, other} = ActionLog.claim_issue("indexable-inc/ix", 3880, loser, log)
    assert other.claimer == nil

    # A pickup-born row was never on offer, so each claim wrote exactly one
    # claimed event (no posted), and the watermark cursor sees exactly the
    # events past it, oldest first -- the idempotent re-claim added nothing.
    assert [first, second] = ActionLog.request_events_after(0, log)
    assert %{event: :claimed, request_id: claimed_id, ref: "indexable-inc/index#3880"} = first
    assert claimed_id == claim.id
    assert %{event: :claimed, ref: "indexable-inc/ix#3880"} = second
    assert [^second] = ActionLog.request_events_after(first.id, log)
    assert ActionLog.last_request_event_id(log) == second.id
  end

  test "a nil-session re-claim is a loss, never a win (#3903)" do
    log = start_supervised!({ActionLog, path: ":memory:", name: :action_log_nil_claims})

    assert {:ok, claim} = ActionLog.claim_issue("indexable-inc/index", 3903, nil, log)
    assert claim.claimed_by == nil

    # nil is every anonymous caller at once, so it can never prove the
    # standing claim is the caller's own: the retry-across-restart carve-out
    # that lets a holder re-claim its own issue must not apply, or two
    # anonymous sessions would both walk away believing they won.
    assert {:error, standing} = ActionLog.claim_issue("indexable-inc/index", 3903, nil, log)
    assert standing.id == claim.id
  end

  test "a request walks post -> claim -> done, events in the same transactions (#3883)" do
    log = start_supervised!({ActionLog, path: ":memory:", name: :action_log_requests})

    poster = ActionLog.create_session("poster", log)
    worker = ActionLog.create_session("worker", log)
    rival = ActionLog.create_session("rival", log)

    assert {:ok, request} =
             ActionLog.post_request(:adhoc, nil, "review the diff", "the body", poster, log)

    assert %{
             kind: :adhoc,
             ref: nil,
             title: "review the diff",
             body: "the body",
             status: :open,
             poster: "poster",
             claimed_by: nil
           } = request

    # Two adhoc posts with the same title are two offers: NULL refs never
    # unique-conflict.
    assert {:ok, twin} = ActionLog.post_request(:adhoc, nil, "review the diff", nil, poster, log)
    assert twin.id != request.id

    # The guarded UPDATE's row count decides the race: the winner flips the
    # row, the loser reads the standing claim back, and the holder's own
    # re-claim stays a win (#3903).
    assert {:ok, claimed} = ActionLog.claim_request(request.id, worker, log)
    assert %{status: :claimed, claimer: "worker"} = claimed
    assert {:ok, ^claimed} = ActionLog.claim_request(request.id, worker, log)
    assert {:error, standing} = ActionLog.claim_request(request.id, rival, log)
    assert standing.claimer == "worker"
    assert {:error, :not_found} = ActionLog.claim_request(999_999, rival, log)

    # Done requires claimed: finishing open work is refused, finishing done
    # work is idempotent.
    assert {:error, %{status: :open}} = ActionLog.finish_request(twin.id, worker, log)
    assert {:ok, done} = ActionLog.finish_request(request.id, worker, log)
    assert %{status: :done} = done
    assert {:ok, %DateTime{}, 0} = DateTime.from_iso8601(done.done_at)
    assert {:ok, ^done} = ActionLog.finish_request(request.id, worker, log)
    assert {:error, :not_found} = ActionLog.finish_request(999_999, worker, log)

    # Exactly one event per mutation, oldest first: two posts, one claim,
    # one done -- the losses and idempotent retries added nothing.
    assert [
             %{
               event: :posted,
               request_id: posted_id,
               session: "poster",
               title: "review the diff"
             },
             %{event: :posted, request_id: twin_id},
             %{event: :claimed, session: "worker"},
             %{event: :done, session: "worker"}
           ] = ActionLog.request_events_after(0, log)

    assert posted_id == request.id
    assert twin_id == twin.id

    # The board lists open first, then claimed, then done.
    assert [%{id: open_id, status: :open}, %{status: :done}] = ActionLog.list_requests(log)
    assert open_id == twin.id
  end

  test "an issue-kind post is an idempotent ensure on the unique ref (#3883)" do
    log = start_supervised!({ActionLog, path: ":memory:", name: :action_log_request_ensure})

    poster = ActionLog.create_session("poster", log)
    worker = ActionLog.create_session("worker", log)

    assert {:ok, request} =
             ActionLog.post_request(
               :issue,
               "indexable-inc/index#3883",
               "generalize pickup",
               nil,
               poster,
               log
             )

    assert %{kind: :issue, ref: "indexable-inc/index#3883", status: :open} = request

    # Re-posting the ref reads the standing row back, whatever its status,
    # and announces nothing new.
    assert {:ok, ^request} =
             ActionLog.post_request(
               :issue,
               "indexable-inc/index#3883",
               "another title",
               nil,
               worker,
               log
             )

    # A pickup of the posted issue claims the SAME row: the veneer and the
    # board are one table.
    assert {:ok, claimed} = ActionLog.claim_issue("indexable-inc/index", 3883, worker, log)
    assert claimed.id == request.id
    assert claimed.title == "generalize pickup"

    assert [%{event: :posted}, %{event: :claimed}] = ActionLog.request_events_after(0, log)
  end

  test "a v7 database folds its issue claims into requests and drops the table (#3883)" do
    path = tmp_db()

    # The exact v7 shape (#3881) with a standing claim, as a pre-#3883
    # binary leaves it on disk.
    {:ok, conn} = Sqlite3.open(path)

    for statement <- [
          "CREATE TABLE sessions (id INTEGER PRIMARY KEY, name TEXT, started_at TEXT NOT NULL, last_seen_at TEXT)",
          "CREATE TABLE topics (id INTEGER PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES sessions(id), name TEXT NOT NULL, started_at TEXT NOT NULL)",
          "CREATE TABLE actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, session_id INTEGER NOT NULL REFERENCES sessions(id), topic_id INTEGER REFERENCES topics(id), tool TEXT NOT NULL, intent TEXT, arguments TEXT NOT NULL, is_error INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'done', stack TEXT, stack_at TEXT, line INTEGER)",
          "CREATE TABLE jobs (id TEXT PRIMARY KEY, session_id INTEGER REFERENCES sessions(id), action_id INTEGER REFERENCES actions(id), intent TEXT, session_name TEXT, topic_name TEXT, code TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'running', watch INTEGER NOT NULL DEFAULT 0, result TEXT, output_bytes INTEGER NOT NULL DEFAULT 0, output_dropped INTEGER NOT NULL DEFAULT 0, started_at TEXT NOT NULL, finished_at TEXT, elapsed_ms INTEGER)",
          "CREATE TABLE job_output (job_id TEXT NOT NULL REFERENCES jobs(id), seq INTEGER NOT NULL, chunk TEXT NOT NULL, PRIMARY KEY (job_id, seq))",
          "CREATE TABLE outbox (id INTEGER PRIMARY KEY, job_id TEXT, intent TEXT, status TEXT NOT NULL, elapsed_ms INTEGER, result TEXT, created_at TEXT NOT NULL, acked INTEGER NOT NULL DEFAULT 0)",
          "CREATE TABLE issue_claims (id INTEGER PRIMARY KEY, repo TEXT NOT NULL, number INTEGER NOT NULL, session_id INTEGER REFERENCES sessions(id), claimed_at TEXT NOT NULL, UNIQUE(repo, number))",
          "CREATE TABLE session_messages (id INTEGER PRIMARY KEY, from_session INTEGER NOT NULL REFERENCES sessions(id), to_session INTEGER REFERENCES sessions(id), body TEXT NOT NULL, created_at TEXT NOT NULL)",
          "INSERT INTO sessions (id, name, started_at, last_seen_at) VALUES (1, 'claimant', '2026-07-20T10:00:00Z', NULL)",
          "INSERT INTO issue_claims (repo, number, session_id, claimed_at) VALUES ('indexable-inc/index', 3880, 1, '2026-07-20T10:00:01Z')",
          "PRAGMA user_version = 7"
        ] do
      :ok = Sqlite3.execute(conn, statement)
    end

    :ok = Sqlite3.close(conn)

    log = start_supervised!({ActionLog, path: path, name: :action_log_v7_to_v8})

    # The claim crossed over as a claimed issue-kind request (the ref doubles
    # as the title -- the claim never carried one, and nobody posted it)...
    assert [request] = ActionLog.list_requests(log)

    assert %{
             kind: :issue,
             ref: "indexable-inc/index#3880",
             title: "indexable-inc/index#3880",
             status: :claimed,
             posted_by: nil,
             claimed_by: 1,
             claimer: "claimant",
             claimed_at: "2026-07-20T10:00:01Z"
           } = request

    # ...still held against a rival claim, with no synthesized events (the
    # feed announces news; a migrated claim was already heard).
    assert {:error, %{claimer: "claimant"}} =
             ActionLog.claim_issue("indexable-inc/index", 3880, nil, log)

    assert ActionLog.request_events_after(0, log) == []

    # The old table is gone and the stamp moved.
    assert user_version(path) == 8

    {:ok, conn} = Sqlite3.open(path)

    {:ok, statement} =
      Sqlite3.prepare(conn, "SELECT name FROM sqlite_master WHERE name = 'issue_claims'")

    {:ok, leftovers} = Sqlite3.fetch_all(conn, statement)
    :ok = Sqlite3.release(conn, statement)
    :ok = Sqlite3.close(conn)
    assert leftovers == []
  end

  test "the message cursor delivers addressed mail and broadcasts, never own sends (#3881)" do
    log = start_supervised!({ActionLog, path: ":memory:", name: :action_log_messages})

    alice = ActionLog.create_session("alice", log)
    bob = ActionLog.create_session("bob", log)
    carol = ActionLog.create_session(nil, log)

    assert ActionLog.last_session_message_id(log) == 0

    assert {:ok, direct} = ActionLog.send_session_message(alice, bob, "for bob", log)
    assert %{from: "alice", from_session: ^alice, to_session: ^bob, body: "for bob"} = direct
    assert {:ok, %DateTime{}, 0} = DateTime.from_iso8601(direct.created_at)

    assert {:ok, blast} = ActionLog.send_session_message(carol, nil, "for everyone", log)
    assert %{from: nil, to_session: nil} = blast

    # Bob hears both; carol hears nothing (the direct row is not hers and a
    # sender never hears her own broadcast); alice hears the broadcast only.
    assert [%{body: "for bob"}, %{body: "for everyone"}] =
             ActionLog.session_messages_after(0, bob, log)

    assert [] = ActionLog.session_messages_after(0, carol, log)
    assert [%{body: "for everyone", from: nil}] = ActionLog.session_messages_after(0, alice, log)

    # The watermark cursor sees exactly the rows past it.
    assert [%{body: "for everyone"}] = ActionLog.session_messages_after(direct.id, bob, log)
    assert ActionLog.last_session_message_id(log) == blast.id
  end

  test "the heartbeat stamps the directory and the directory joins the current topic (#3881)" do
    log = start_supervised!({ActionLog, path: ":memory:", name: :action_log_directory})

    quiet = ActionLog.create_session("quiet", log)
    busy = ActionLog.create_session("busy", log)
    _first = ActionLog.create_topic(busy, "warmup", log)
    _second = ActionLog.create_topic(busy, "the real work", log)

    assert [%{id: ^quiet, last_seen_at: nil, topic: nil}, before_beat] =
             ActionLog.session_directory(log)

    assert %{id: ^busy, name: "busy", topic: "the real work", last_seen_at: nil} = before_beat

    :ok = ActionLog.heartbeat_session(busy, log)
    [_quiet, beaten] = ActionLog.session_directory(log)
    assert {:ok, %DateTime{}, 0} = DateTime.from_iso8601(beaten.last_seen_at)
  end

  test "a database stamped by a newer server disables logging instead of crashing" do
    path = tmp_db()

    {:ok, conn} = Sqlite3.open(path)
    :ok = Sqlite3.execute(conn, "PRAGMA user_version = 9000")
    :ok = Sqlite3.close(conn)

    {log, output} =
      with_log(fn ->
        start_supervised!({ActionLog, path: path, name: :action_log_future})
      end)

    # The refusal names both versions, so the operator knows which side moves.
    assert output =~ "user_version 9000"
    assert output =~ "supported 8"
    assert output =~ "index#3539"

    # The server stays useful: writes are absorbed, reads answer empty.
    session_id = ActionLog.create_session("ignored", log)
    topic_id = ActionLog.create_topic(session_id, "t", log)

    action_id =
      ActionLog.start_action(
        %{session_id: session_id, topic_id: topic_id, tool: "exec", intent: "x", arguments: "{}"},
        log
      )

    :ok = ActionLog.update_stack(action_id, "[]", nil, log)
    :ok = ActionLog.finish_action(action_id, "done", false, 1, log)
    assert ActionLog.recent(10, log) == []
    assert ActionLog.sessions(log) == []
    assert ActionLog.topics(log) == []

    # With no arbiter there is no claim to win, and no bus to post on (#3883).
    assert ActionLog.claim_issue("indexable-inc/index", 1, session_id, log) == :disabled
    assert ActionLog.post_request(:adhoc, nil, "t", nil, session_id, log) == :disabled
    assert ActionLog.claim_request(1, session_id, log) == :disabled
    assert ActionLog.finish_request(1, session_id, log) == :disabled
    assert ActionLog.list_requests(log) == []
    assert ActionLog.request_events_after(0, log) == []
    assert ActionLog.last_request_event_id(log) == 0

    # And no bus to carry a message (#3881).
    assert ActionLog.heartbeat_session(session_id, log) == :ok
    assert ActionLog.session_directory(log) == []
    assert ActionLog.send_session_message(session_id, nil, "hello?", log) == :disabled
    assert ActionLog.session_messages_after(0, session_id, log) == []
    assert ActionLog.last_session_message_id(log) == 0

    # The newer file is left exactly as found, for the newer server.
    assert user_version(path) == 9000

    {:ok, conn} = Sqlite3.open(path)
    {:ok, statement} = Sqlite3.prepare(conn, "SELECT name FROM sqlite_master")
    {:ok, tables} = Sqlite3.fetch_all(conn, statement)
    :ok = Sqlite3.release(conn, statement)
    :ok = Sqlite3.close(conn)
    assert tables == []
  end

  # Several server instances share one database file, so a write can land
  # while a sibling holds the write lock. SQLite reports that as :busy, which
  # used to badmatch in run/3 and kill the log and the calling job (#3890).
  test "a write blocked by a sibling's transaction waits the lock out instead of crashing" do
    path = tmp_db()

    # The wide busy bound is headroom, not the expected wait: the release
    # lands ~300ms in, but a loaded sandbox can starve the releasing task
    # for seconds, and with the 5s default the write's busy wait expired
    # first and the test died on the caller's bound (#3903). It only has
    # to stay below call/3's 30s timeout so a truly stuck lock still fails
    # as the server's descriptive raise.
    log =
      start_supervised!(
        {ActionLog, path: path, name: :action_log_busy_wait, busy_timeout_ms: 20_000}
      )

    session_id = ActionLog.create_session("busy", log)

    {:ok, blocker} = Sqlite3.open(path)
    :ok = Sqlite3.execute(blocker, "BEGIN IMMEDIATE")

    release =
      Task.async(fn ->
        Process.sleep(300)
        :ok = Sqlite3.execute(blocker, "ROLLBACK")
      end)

    assert :ok = ActionLog.rename_session(session_id, "survived", log)
    assert Task.await(release) == :ok
    :ok = Sqlite3.close(blocker)
  end

  test "a lock outliving the bounded busy wait fails loudly, not with a badmatch (#3890)" do
    path = tmp_db()

    log =
      start_supervised!(
        {ActionLog, path: path, name: :action_log_busy_bound, busy_timeout_ms: 20}
      )

    {:ok, blocker} = Sqlite3.open(path)
    :ok = Sqlite3.execute(blocker, "BEGIN IMMEDIATE")
    ref = Process.monitor(log)

    capture_log(fn ->
      assert catch_exit(ActionLog.create_session("never", log))
      assert_receive {:DOWN, ^ref, :process, _pid, {%RuntimeError{message: message}, _stack}}
      assert message =~ "busy"
      assert message =~ "3890"
    end)

    :ok = Sqlite3.execute(blocker, "ROLLBACK")
    :ok = Sqlite3.close(blocker)
  end

  # Under load a job's `job_started` write can be absorbed by the job's
  # ledger seam (#3874) and never land. The terminal transition used to
  # answer `:already_final` for the missing row, erasing the run from the
  # record entirely (#4082); with the start metadata it reconstructs the
  # row inside the same transition.
  test "finish_job reconstructs a jobs row whose start write never landed (#4082)" do
    path = tmp_db()
    log = start_supervised!({ActionLog, path: path, name: :action_log_4082_ghost})
    session_id = ActionLog.create_session("ghost", log)

    start = %{
      id: "gh4082",
      session_id: session_id,
      action_id: nil,
      intent: "ghost",
      session_name: "ghost",
      topic_name: nil,
      code: ":ok",
      watch: false,
      started_at: DateTime.to_iso8601(DateTime.utc_now())
    }

    # No job_started call: the start write was lost.
    assert {:notify, outbox} =
             ActionLog.finish_job("gh4082", :done, ":ok", [start: start], log)

    refute outbox.acked
    assert %{status: :done, code: ":ok", intent: "ghost"} = ActionLog.job("gh4082", log)
    assert [%{id: "gh4082"}] = ActionLog.recent_jobs(session_id, 10, log)

    # The guard still decides the race: a second finalize no-ops.
    assert ActionLog.finish_job("gh4082", :killed, "late", [start: start], log) ==
             :already_final
  end
end
