defmodule IxMcp.ActionLogTest do
  use ExUnit.Case, async: false

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

    :ok =
      ActionLog.record(
        %{
          session_id: session_id,
          topic_id: topic_id,
          tool: "exec",
          intent: "persist me",
          arguments: "{}",
          is_error: false,
          elapsed_ms: 7
        },
        log
      )

    assert [%{intent: "persist me"}] = ActionLog.recent(20, log)
    stop_supervised!(:first_open)

    reopened =
      start_supervised!({ActionLog, path: path, name: :action_log_reopen_test}, id: :reopen)

    assert [%{intent: "persist me", tool: "exec", session: "s", topic: "t"}] =
             ActionLog.recent(20, reopened)
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
             "elapsed_ms"
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
end
