defmodule IxMcp.ActionLogBusyTest do
  @moduledoc """
  Regression for #3874: several server instances share one action-log
  database, so a sibling holding the write lock is normal operation. Ledger
  writes must wait it out; before the fix they match-crashed the GenServer
  (`{:badmatch, :busy}` in start_action, `{:badmatch, {:error, "database is
  locked"}}` in the job-ledger transaction).
  """
  use ExUnit.Case, async: true

  alias Exqlite.Sqlite3
  alias IxMcp.ActionLog

  @moduletag :tmp_dir

  setup %{tmp_dir: dir} do
    path = Path.join(dir, "actions.db")
    name = :"action_log_busy_#{System.unique_integer([:positive])}"
    log = start_supervised!({ActionLog, path: path, name: name})

    # A second connection standing in for a concurrent server instance.
    {:ok, holder} = Sqlite3.open(path)

    %{log: log, name: name, holder: holder}
  end

  test "start_action waits out a held write lock instead of crashing", ctx do
    sid = ActionLog.create_session("busy", ctx.name)
    :ok = Sqlite3.execute(ctx.holder, "BEGIN IMMEDIATE")

    task =
      Task.async(fn ->
        ActionLog.start_action(
          %{session_id: sid, topic_id: nil, tool: "exec", intent: "busy", arguments: "{}"},
          ctx.name
        )
      end)

    # Let the insert actually hit the held lock before releasing it.
    Process.sleep(150)
    :ok = Sqlite3.execute(ctx.holder, "COMMIT")

    assert is_integer(Task.await(task, 10_000))
    assert Process.alive?(ctx.log)
  end

  test "the job-ledger transaction waits out a held write lock", ctx do
    :ok = ActionLog.job_started(job_start("j1"), ctx.name)
    :ok = Sqlite3.execute(ctx.holder, "BEGIN IMMEDIATE")

    task = Task.async(fn -> ActionLog.append_job_output("j1", [{1, "chunk"}], 0, ctx.name) end)

    Process.sleep(150)
    :ok = Sqlite3.execute(ctx.holder, "COMMIT")

    assert :ok = Task.await(task, 10_000)
    assert Process.alive?(ctx.log)
    assert ActionLog.job_output("j1", ctx.name) == "chunk"
  end

  defp job_start(id) do
    %{
      id: id,
      session_id: nil,
      action_id: nil,
      intent: nil,
      session_name: nil,
      topic_name: nil,
      code: ":ok",
      watch: false,
      started_at: DateTime.to_iso8601(DateTime.utc_now())
    }
  end
end
