defmodule TuiExTest do
  # Not async: every terminal lives in one process-global manager; keep the
  # spawns ordered so screens and exit states stay deterministic.
  use ExUnit.Case, async: false

  import UnibindTest.Eventually

  # PTY exit propagation crosses the OS and the NIF driver; give it more
  # room than the shared 2s default.
  @exit_timeout_ms 5_000

  test "spawn + send + wait_for round-trips through a real PTY" do
    {:ok, id} = TuiEx.spawn("cat", [])
    assert id in TuiEx.list()
    :ok = TuiEx.send(id, "hello-pty\r")
    {:ok, screen} = TuiEx.wait_for(id, "hello-pty")
    assert screen =~ "hello-pty"
    {:ok, snap} = TuiEx.snapshot(id)
    assert snap =~ "hello-pty"
    assert TuiEx.is_alive(id) == {:ok, true}
    :ok = TuiEx.close(id)
    refute id in TuiEx.list()
  end

  test "spawn geometry reaches the child as its terminal size" do
    # stty reads the PTY's window size, so `5 20` on screen proves the
    # rows/cols arguments plumbed through to the kernel PTY.
    {:ok, id} = TuiEx.spawn("stty", ["size"], 5, 20)
    {:ok, screen} = TuiEx.wait_for(id, "5 20")
    assert screen =~ "5 20"
    :ok = TuiEx.close(id)
  end

  test "named keys drive the child: enter submits, ctrl+d ends the stream" do
    {:ok, id} = TuiEx.spawn("cat", [])
    :ok = TuiEx.send(id, "abc")
    :ok = TuiEx.send_key(id, "enter")
    # PTY echo shows the typed line once; cat writes it back after Enter.
    {:ok, screen} = TuiEx.wait_for(id, "abc\nabc")
    assert screen =~ "abc\nabc"
    :ok = TuiEx.send_key(id, "ctrl+d")

    assert eventually(fn -> TuiEx.is_alive(id) == {:ok, false} end, @exit_timeout_ms),
           "cat did not exit on ctrl+d"

    assert TuiEx.exit_code(id) == {:ok, 0}
    :ok = TuiEx.close(id)
  end

  test "a shell one-liner's exit code surfaces through wait and exit_code" do
    {:ok, id} = TuiEx.spawn("sh", ["-c", "exit 7"])
    assert {:ok, 7} = TuiEx.wait(id)
    assert TuiEx.exit_code(id) == {:ok, 7}
    assert TuiEx.is_alive(id) == {:ok, false}
    :ok = TuiEx.close(id)
  end

  test "an interactive read loop answers typed input" do
    {:ok, id} = TuiEx.spawn("sh", ["-c", "read line; echo got:$line"])
    :ok = TuiEx.send(id, "ping")
    :ok = TuiEx.send_key(id, "enter")
    {:ok, screen} = TuiEx.wait_for(id, "got:ping")
    assert screen =~ "got:ping"
    :ok = TuiEx.close(id)
  end

  test "kill terminates a child that would run forever" do
    {:ok, id} = TuiEx.spawn("cat", [])
    assert TuiEx.is_alive(id) == {:ok, true}
    :ok = TuiEx.kill(id)

    assert eventually(fn -> TuiEx.is_alive(id) == {:ok, false} end, @exit_timeout_ms),
           "cat survived SIGKILL"

    # A signal death has no exit code.
    assert TuiEx.exit_code(id) == {:ok, nil}
    :ok = TuiEx.close(id)
  end

  test "wait_for times out with the :timeout variant" do
    {:ok, id} = TuiEx.spawn("cat", [])

    assert {:error, %TuiEx.TuiError{variant: :timeout}} =
             TuiEx.wait_for(id, "never-appears", 200)

    :ok = TuiEx.close(id)
  end

  test "unknown key names are rejected, not sent" do
    {:ok, id} = TuiEx.spawn("cat", [])

    assert {:error, %TuiEx.TuiError{variant: :bad_key}} = TuiEx.send_key(id, "warp")
    assert {:error, %TuiEx.TuiError{variant: :bad_key}} = TuiEx.send_key(id, "ctrl+42")

    :ok = TuiEx.close(id)
  end

  test "operations on an unknown id carry the :not_found variant" do
    assert {:error, %TuiEx.TuiError{variant: :not_found}} = TuiEx.snapshot("nope")
    assert {:error, %TuiEx.TuiError{variant: :not_found}} = TuiEx.send("nope", "x")
    assert {:error, %TuiEx.TuiError{variant: :not_found}} = TuiEx.kill("nope")
  end
end
