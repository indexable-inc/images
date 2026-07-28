defmodule IxMcp.ActionLogBusyTest do
  @moduledoc """
  Regression for #3874: several server instances share one action-log
  database, so a sibling holding the write lock is normal operation. #3890
  landed the busy-timeout wait itself (see the sibling tests in
  action_log_test.exs); these tests pin the two write paths the incident
  crashed that those tests do not touch -- the prepared start_action insert
  (`{:badmatch, :busy}`) and the job-ledger BEGIN IMMEDIATE transaction
  (`{:badmatch, {:error, "database is locked"}}` through Sqlite3.execute).
  """
  use ExUnit.Case, async: true

  alias Exqlite.Sqlite3
  alias IxMcp.ActionLog

  @moduletag :tmp_dir

  setup %{tmp_dir: dir} do
    path = Path.join(dir, "actions.db")
    name = :"action_log_busy_#{System.unique_integer([:positive])}"

    # 20s of busy headroom (#3903): the sibling releases its lock ~150ms in,
    # but a loaded sandbox can starve the releasing task for seconds, and
    # the 5s default left the write racing that skew. The bound stays below
    # call/3's 30s timeout so a truly stuck lock still fails loudly.
    log = start_supervised!({ActionLog, path: path, name: name, busy_timeout_ms: 20_000})

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

    assert is_integer(Task.await(task, 30_000))
    assert Process.alive?(ctx.log)
  end

  test "the job-ledger transaction waits out a held write lock", ctx do
    :ok = ActionLog.job_started(job_start("j1"), ctx.name)
    :ok = Sqlite3.execute(ctx.holder, "BEGIN IMMEDIATE")

    task = Task.async(fn -> ActionLog.append_job_output("j1", [{1, "chunk"}], 0, ctx.name) end)

    Process.sleep(150)
    :ok = Sqlite3.execute(ctx.holder, "COMMIT")

    assert :ok = Task.await(task, 30_000)
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
