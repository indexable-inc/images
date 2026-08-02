defmodule LoomTest do
  use ExUnit.Case, async: false

  alias Loom.FakeIx

  setup context do
    FakeIx.setup(context)
  end

  test "spawn runs snapshot -> new -> shell(claude -p) and delivers the final text", ctx do
    {:ok, id} = Loom.spawn("do the thing", parent_vm: "ctl")
    vm = "loom-#{id}"

    assert_receive {:loom, ^id, {:spawned, ^vm}}, 5_000
    assert_receive {:loom, ^id, {:final, "the final answer"}}, 5_000
    assert_receive {:loom, ^id, :stopped}, 5_000

    [snapshot, new, shell, sync, stop] = FakeIx.await_calls(ctx[:calls_log], 5)
    assert sync == ["shell", vm, "--noninteractive", "--", "sync"]
    assert snapshot == ["snapshot", "ctl"]
    assert new == ["new", FakeIx.snapshot_id(), "--name", vm, "--no-shell"]

    assert shell == [
             "shell",
             vm,
             "--noninteractive",
             "--",
             "claude",
             "-p",
             "do the thing",
             "--output-format",
             "stream-json",
             "--verbose"
           ]

    assert stop == ["stop", vm]

    {:ok, status} = Loom.status(id)
    assert status.phase == :idle
    assert status.session_id == "sess-fixture-1"
    assert status.result == "the final answer"
  end

  test "send to an idle agent wakes the VM and resumes the session", ctx do
    {:ok, id} = Loom.spawn("first turn", parent_vm: "ctl")
    assert_receive {:loom, ^id, {:final, _text}}, 5_000
    assert_receive {:loom, ^id, :stopped}, 5_000
    vm = "loom-#{id}"

    assert :ok = Loom.send_text(id, "second turn")
    assert_receive {:loom, ^id, :woken}, 5_000
    assert_receive {:loom, ^id, {:final, "the final answer"}}, 5_000

    calls = FakeIx.await_calls(ctx[:calls_log], 9)
    verbs = Enum.map(calls, &hd/1)

    assert verbs == [
             "snapshot",
             "new",
             "shell",
             "shell",
             "stop",
             "start",
             "shell",
             "shell",
             "stop"
           ]

    resume = Enum.at(calls, 6)

    assert resume == [
             "shell",
             vm,
             "--noninteractive",
             "--",
             "claude",
             "-p",
             "--resume",
             "sess-fixture-1",
             "second turn",
             "--output-format",
             "stream-json",
             "--verbose"
           ]
  end

  test "send while running refuses with :busy", ctx do
    # A stream that never terminates on its own: point the shell at a
    # fifo so the child stays running until we delete the agent.
    fifo = Path.join(ctx[:fake_dir], "stream.fifo")
    {_out, 0} = System.cmd("mkfifo", [fifo])
    System.put_env("LOOM_FAKE_STREAM_FILE", fifo)

    {:ok, id} = Loom.spawn("long task", parent_vm: "ctl")
    assert_receive {:loom, ^id, {:spawned, _vm}}, 5_000

    # Provisioned and streaming (the port is open on the fifo).
    assert {:error, :busy} = Loom.send_text(id, "impatient follow-up")

    # Unblock the fifo writer side and tear down.
    writer = File.open!(fifo, [:write])
    IO.write(writer, "")
    File.close(writer)
    assert :ok = Loom.delete(id)
    assert Loom.status(id) == {:error, :not_found}
  end

  test "a failed snapshot fails the agent and runs nothing further", ctx do
    System.put_env("LOOM_FAKE_FAIL", "snapshot")

    {:ok, id} = Loom.spawn("doomed", parent_vm: "ctl")
    assert_receive {:loom, ^id, {:failed, {:provision, {:exit, 1, _message}}, _tail}}, 5_000

    calls = FakeIx.await_calls(ctx[:calls_log], 1)
    assert Enum.map(calls, &hd/1) == ["snapshot"]

    {:ok, status} = Loom.status(id)
    assert status.phase == :failed
  end

  test "a child that exits non-zero fails the agent", ctx do
    System.put_env("LOOM_FAKE_STREAM_EXIT", "3")
    on_exit(fn -> System.delete_env("LOOM_FAKE_STREAM_EXIT") end)

    {:ok, id} = Loom.spawn("crashy", parent_vm: "ctl")
    assert_receive {:loom, ^id, {:failed, {:child_exit, 3}, log_tail}}, 5_000
    assert is_list(log_tail) and log_tail != [], "failure must carry the child's output"

    # No stop after a failed child: the VM is left up for inspection.
    calls = FakeIx.await_calls(ctx[:calls_log], 3)
    assert Enum.map(calls, &hd/1) == ["snapshot", "new", "shell"]
  end

  test "spawn without a parent VM refuses" do
    assert {:error, :no_parent_vm} = Loom.spawn("nowhere to fork from")
  end

  test "a configured preflight gates the child launch", ctx do
    Application.put_env(:loom, :preflight, "test -s /run/secrets/anthropic_api_key")
    on_exit(fn -> Application.delete_env(:loom, :preflight) end)

    {:ok, id} = Loom.spawn("gated", parent_vm: "ctl")
    assert_receive {:loom, ^id, {:final, "the final answer"}}, 5_000

    calls = FakeIx.await_calls(ctx[:calls_log], 6)
    verbs = Enum.map(calls, &hd/1)
    assert verbs == ["snapshot", "new", "shell", "shell", "shell", "stop"]

    # The preflight is the FIRST shell call, before the claude child.
    assert Enum.at(calls, 2) == [
             "shell",
             "loom-#{id}",
             "--noninteractive",
             "--",
             "sh",
             "-c",
             "test -s /run/secrets/anthropic_api_key"
           ]
  end
end
