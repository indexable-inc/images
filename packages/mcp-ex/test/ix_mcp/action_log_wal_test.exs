defmodule IxMcp.ActionLogWalTest do
  @moduledoc """
  Regression for #4092, the 2026-07-23 parallel-session incident: several kernels
  share one action-log file, and under the default rollback journal a
  sibling's open read transaction holds the lock every write needs. Writes
  outlived the busy budget, every instance's log crash-looped, and two
  kernels died wholesale through restart intensity, closing their MCP
  connections mid-call. WAL keeps readers and the writer independent, so a
  held read snapshot must not block writes at all.
  """
  use ExUnit.Case, async: true

  alias Exqlite.Sqlite3
  alias IxMcp.ActionLog

  @moduletag :tmp_dir

  test "a sibling's open read transaction does not block writes", %{tmp_dir: dir} do
    path = Path.join(dir, "actions.db")
    name = :"action_log_wal_#{System.unique_integer([:positive])}"

    # The short busy budget is the point: under the rollback journal the
    # write below sat blocked behind the reader until this budget expired
    # and the log crashed; under WAL it must not wait at all.
    log = start_supervised!({ActionLog, path: path, name: name, busy_timeout_ms: 500})

    # A concurrent server instance parked in a read transaction -- a
    # history/recent_jobs scan over a grown log holds its lock long enough
    # for sibling writes to pile into it.
    {:ok, holder} = Sqlite3.open(path)
    :ok = Sqlite3.execute(holder, "BEGIN")
    {:ok, stmt} = Sqlite3.prepare(holder, "SELECT count(*) FROM sessions")
    {:ok, _rows} = Sqlite3.fetch_all(holder, stmt)

    assert is_integer(ActionLog.create_session("wal", name))
    assert Process.alive?(log)

    :ok = Sqlite3.execute(holder, "COMMIT")
    :ok = Sqlite3.close(holder)
  end
end
