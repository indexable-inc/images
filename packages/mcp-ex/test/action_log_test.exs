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
    stack =
      poll_until(fn ->
        case ActionLog.recent(1) do
          [%{status: "running", stack: stack} | _] when is_binary(stack) -> stack
          _ -> nil
        end
      end)

    frames = JSON.decode!(stack)
    assert Enum.any?(frames, &(&1 =~ "sleep")), "expected a sleep frame in #{inspect(frames)}"

    :ok = IxMcp.Jobs.cancel(job_id)

    assert [entry | _] = ActionLog.recent(1)
    assert entry.status == "cancelled"
    refute entry.is_error
    assert entry.stack == nil, "finalizing clears the sampled stack"
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

  test "one server instance is one session; topic_set is a timeline, not a dictionary" do
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

    :ok = ActionLog.update_stack(action_id, ~s(["frame"]), log)
    assert [%{status: "running", stack: ~s(["frame"])} | _] = ActionLog.recent(10, log)
    :ok = ActionLog.finish_action(action_id, "done", false, 3, log)
    assert [%{status: "done", stack: nil} | _] = ActionLog.recent(10, log)

    # The 2 -> 3 step stamped the file on its way through (index#3539).
    assert user_version(path) == 3
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
    assert length(entries) == 6

    assert Enum.map(entries, &{&1.session, &1.topic, &1.tool, &1.elapsed_ms}) == [
             {"beta", "build", "elixir_exec", 6},
             {"alpha", "build", "kernel_trace", 5},
             {"alpha", "build", "elixir_exec", 4},
             {"alpha", nil, "elixir_exec", 3},
             {nil, "loose", "read", 2},
             {nil, nil, "elixir_exec", 1}
           ]

    stop_supervised!(:migrate)

    # The ladder ran 1 -> 2 -> 3 and left the stamp behind (index#3539).
    assert user_version(path) == 3

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
             "stack_at"
           ]

    {:ok, statement} =
      Sqlite3.prepare(
        conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name"
      )

    {:ok, tables} = Sqlite3.fetch_all(conn, statement)
    :ok = Sqlite3.release(conn, statement)
    assert tables == [["actions"], ["sessions"], ["topics"]]
    :ok = Sqlite3.close(conn)

    reopened = start_supervised!({ActionLog, path: path, name: :action_log_remigration})
    assert length(ActionLog.sessions(reopened)) == 3
    assert length(ActionLog.recent(10, reopened)) == 6
  end

  test "a fresh database is created stamped with the current schema version" do
    path = tmp_db()
    start_supervised!({ActionLog, path: path, name: :action_log_fresh_stamp})
    assert user_version(path) == 3
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
    assert user_version(path) == 3
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
    assert output =~ "supported 3"
    assert output =~ "index#3539"

    # The server stays useful: writes are absorbed, reads answer empty.
    session_id = ActionLog.create_session("ignored", log)
    topic_id = ActionLog.create_topic(session_id, "t", log)

    action_id =
      ActionLog.start_action(
        %{session_id: session_id, topic_id: topic_id, tool: "exec", intent: "x", arguments: "{}"},
        log
      )

    :ok = ActionLog.update_stack(action_id, "[]", log)
    :ok = ActionLog.finish_action(action_id, "done", false, 1, log)
    assert ActionLog.recent(10, log) == []
    assert ActionLog.sessions(log) == []
    assert ActionLog.topics(log) == []

    # The newer file is left exactly as found, for the newer server.
    assert user_version(path) == 9000

    {:ok, conn} = Sqlite3.open(path)
    {:ok, statement} = Sqlite3.prepare(conn, "SELECT name FROM sqlite_master")
    {:ok, tables} = Sqlite3.fetch_all(conn, statement)
    :ok = Sqlite3.release(conn, statement)
    :ok = Sqlite3.close(conn)
    assert tables == []
  end
end
