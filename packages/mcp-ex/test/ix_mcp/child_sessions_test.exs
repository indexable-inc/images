defmodule IxMcp.ChildSessionsTest do
  use ExUnit.Case, async: false

  alias IxMcp.ActionLog
  alias IxMcp.Sessions

  # Own ActionLog instance for the same reason sessions_test gives: the
  # globally running log has a real SessionWatch sweeping it.
  setup do
    name = :"child_sessions_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: ":memory:", name: name})
    %{log: name}
  end

  defp beat(log, id), do: :ok = ActionLog.heartbeat_session(id, log)

  test "a child row round-trips its parent and a peer stays parentless", %{log: log} do
    lead = ActionLog.create_session("lead", log)
    child = ActionLog.create_session("researcher", log, parent: lead)

    rows = ActionLog.session_directory(log)
    assert %{parent: ^lead} = Enum.find(rows, &(&1.id == child))
    assert %{parent: nil} = Enum.find(rows, &(&1.id == lead))
  end

  test "a spawn tag round-trips, so an outside spawner can find its row", %{log: log} do
    tagged = ActionLog.create_session(nil, log, spawn_tag: "wrapper-abc123")
    plain = ActionLog.create_session(nil, log)

    rows = ActionLog.session_directory(log)
    assert %{spawn_tag: "wrapper-abc123"} = Enum.find(rows, &(&1.id == tagged))
    assert %{spawn_tag: nil} = Enum.find(rows, &(&1.id == plain))
  end

  test "children are hidden from the peer list unless asked for", %{log: log} do
    lead = ActionLog.create_session("lead", log)
    child = ActionLog.create_session("researcher", log, parent: lead)
    beat(log, lead)
    beat(log, child)

    # Hidden by default: the list is who a cell can delegate to, and a
    # registered child has no kernel to read a delegation with.
    peers = Sessions.list(action_log: log, session_id: lead)
    refute Enum.any?(peers, &(&1.id == child))

    named = Sessions.list(action_log: log, session_id: lead, children: true)
    assert %{parent: ^lead} = Enum.find(named, &(&1.id == child))
  end

  test "a send to a child is refused and names the lead's channel", %{log: log} do
    me = ActionLog.create_session("me", log)
    lead = ActionLog.create_session("lead", log)
    child = ActionLog.create_session("researcher", log, parent: lead)
    beat(log, child)

    assert {:error, reason} = Sessions.send(child, "hello?", action_log: log, session_id: me)
    assert reason =~ "subagent of session #{lead}"
    assert reason =~ "Agents.send/2"
  end

  test "a child whose name collides with a peer does not make the peer ambiguous", %{log: log} do
    me = ActionLog.create_session("me", log)
    lead = ActionLog.create_session("lead", log)
    peer = ActionLog.create_session("worker", log)
    _child = ActionLog.create_session("worker", log, parent: lead)
    beat(log, peer)

    assert {:ok, detail} = Sessions.send("worker", "task", action_log: log, session_id: me)
    assert detail =~ "##{peer}"
  end

  test "an old database migrates: pre-registry rows read as parentless peers" do
    # The v1 shape, same minimal fixture action_log_test's migration test
    # builds: booting the log walks the whole ladder, so this exercises the
    # 9 -> 10 step without hand-copying any intermediate schema.
    path =
      Path.join(System.tmp_dir!(), "child_sessions_old_#{System.unique_integer([:positive])}.db")

    on_exit(fn -> File.rm(path) end)

    {:ok, db} = Exqlite.Sqlite3.open(path)

    for statement <- [
          "CREATE TABLE actions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, session TEXT," <>
            " topic TEXT, tool TEXT NOT NULL, intent TEXT, arguments TEXT NOT NULL," <>
            " is_error INTEGER NOT NULL, elapsed_ms INTEGER NOT NULL)",
          "INSERT INTO actions (at, session, topic, tool, intent, arguments, is_error, elapsed_ms)" <>
            " VALUES ('2026-01-01T00:00:00Z', 'elder', NULL, 'exec', NULL, '{}', 0, 1)"
        ] do
      :ok = Exqlite.Sqlite3.execute(db, statement)
    end

    :ok = Exqlite.Sqlite3.close(db)

    name = :"child_sessions_old_log_#{System.unique_integer([:positive])}"
    # The setup block already runs an ActionLog under the default spec id.
    start_supervised!(Supervisor.child_spec({ActionLog, path: path, name: name}, id: name))

    assert [%{name: "elder", parent: nil, spawn_tag: nil}] =
             Enum.filter(ActionLog.session_directory(name), &(&1.name == "elder"))

    lead = ActionLog.create_session("lead", name)
    child = ActionLog.create_session("late-child", name, parent: lead)
    assert %{parent: ^lead} = Enum.find(ActionLog.session_directory(name), &(&1.id == child))
  end
end
