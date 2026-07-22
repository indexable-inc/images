defmodule IxMcp.SessionsTest do
  use ExUnit.Case, async: false

  alias IxMcp.ActionLog
  alias IxMcp.Sessions

  # Own ActionLog instance (in-memory, own name): directory rows and messages
  # must never land in the globally running log, where a leaked
  # IX_MCP_STDIO=1 application boot has a real SessionWatch sweeping (see
  # issue_watch_test.exs for the same gotcha).
  setup do
    name = :"sessions_test_log_#{System.unique_integer([:positive])}"
    start_supervised!({ActionLog, path: ":memory:", name: name})
    %{log: name}
  end

  defp beat(log, id), do: :ok = ActionLog.heartbeat_session(id, log)

  test "list shows heartbeat rows with liveness flags and hides dead history", %{log: log} do
    me = ActionLog.create_session("me", log)
    peer = ActionLog.create_session("peer", log)
    stale = ActionLog.create_session("stale", log)
    _dead = ActionLog.create_session("never-heartbeat", log)

    beat(log, me)
    beat(log, peer)
    beat(log, stale)

    # Liveness is measured against the caller's clock: an hour from now the
    # fresh stamps have gone stale.
    now = DateTime.utc_now()
    later = DateTime.add(now, 3600)

    rows = Sessions.list(action_log: log, session_id: me, now: now)
    assert Enum.map(rows, & &1.name) |> Enum.sort() == ["me", "peer", "stale"]
    assert %{live: true, self: true} = Enum.find(rows, &(&1.id == me))
    assert %{live: true, self: false} = Enum.find(rows, &(&1.id == peer))

    assert Sessions.list(action_log: log, session_id: me, now: later)
           |> Enum.all?(&(not &1.live))

    # all: true exposes the row that never heartbeat.
    assert Sessions.list(action_log: log, session_id: me, now: now, all: true)
           |> Enum.any?(&(&1.name == "never-heartbeat"))
  end

  test "list includes this session even before its first heartbeat", %{log: log} do
    me = ActionLog.create_session("just-born", log)

    assert [%{id: ^me, self: true, live: false}] =
             Sessions.list(action_log: log, session_id: me)
  end

  test "send by id records the message; self and unknown ids are rejected", %{log: log} do
    me = ActionLog.create_session("me", log)
    peer = ActionLog.create_session("peer", log)
    beat(log, peer)

    assert {:ok, detail} = Sessions.send(peer, "take the retro", action_log: log, session_id: me)
    assert detail =~ "sent to session peer (##{peer})"

    assert [%{body: "take the retro", from: "me"}] =
             ActionLog.session_messages_after(0, peer, log)

    assert {:error, message} = Sessions.send(me, "hi me", action_log: log, session_id: me)
    assert message =~ "is this session"

    assert {:error, message} = Sessions.send(999, "hi?", action_log: log, session_id: me)
    assert message =~ "no session 999"
  end

  test "send to a stale target still records, but says so", %{log: log} do
    me = ActionLog.create_session("me", log)
    gone = ActionLog.create_session("gone", log)
    beat(log, gone)

    later = DateTime.utc_now() |> DateTime.add(3600)

    assert {:ok, detail} =
             Sessions.send(gone, "anyone there?", action_log: log, session_id: me, now: later)

    assert detail =~ "heartbeat stopped"
    assert [%{body: "anyone there?"}] = ActionLog.session_messages_after(0, gone, log)
  end

  test "a name resolves to the one live session; ambiguity demands the id", %{log: log} do
    me = ActionLog.create_session("me", log)
    old = ActionLog.create_session("worker", log)
    current = ActionLog.create_session("worker", log)
    beat(log, current)

    # Two rows share the name, but only one is live: unambiguous.
    assert {:ok, _detail} = Sessions.send("worker", "ping", action_log: log, session_id: me)
    assert [%{body: "ping"}] = ActionLog.session_messages_after(0, current, log)
    assert [] = ActionLog.session_messages_after(0, old, log)

    # Both live: the name no longer picks one.
    beat(log, old)
    assert {:error, message} = Sessions.send("worker", "ping", action_log: log, session_id: me)
    assert message =~ "send to the id instead"

    assert {:error, message} = Sessions.send("nobody", "ping", action_log: log, session_id: me)
    assert message =~ ~s(no session named "nobody")
  end

  test "broadcast records a NULL recipient and counts live peers", %{log: log} do
    me = ActionLog.create_session("me", log)
    peer = ActionLog.create_session("peer", log)
    beat(log, me)
    beat(log, peer)

    assert {:ok, detail} = Sessions.broadcast("stand-up in 5", action_log: log, session_id: me)
    assert detail =~ "1 live peer(s)"

    assert [%{body: "stand-up in 5", to_session: nil}] =
             ActionLog.session_messages_after(0, peer, log)
  end
end
