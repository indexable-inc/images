defmodule IxMcp.TuiLocalTest do
  # Not async: terminals share one process-global PTY manager.
  use ExUnit.Case, async: false

  alias IxMcp.TuiLocal

  @moduletag :tui_local

  test "drives a real PTY end to end: spawn, type, read, key, exit" do
    {:ok, term} = TuiLocal.spawn("cat", [], rows: 10, cols: 40)
    assert {:ok, true} = TuiLocal.alive?(term)

    :ok = TuiLocal.send(term, "hello-cell")
    :ok = TuiLocal.send_key(term, "enter")
    {:ok, screen} = TuiLocal.wait_for(term, "hello-cell\nhello-cell")
    assert screen =~ "hello-cell"
    {:ok, snap} = TuiLocal.snapshot(term)
    assert snap =~ "hello-cell"

    :ok = TuiLocal.send_key(term, "ctrl+d")
    assert {:ok, 0} = TuiLocal.wait(term)
    assert {:ok, false} = TuiLocal.alive?(term)
    :ok = TuiLocal.close(term)
  end

  test "wait_for surfaces the :timeout variant" do
    {:ok, term} = TuiLocal.spawn("cat", [])
    assert {:error, %{variant: :timeout}} = TuiLocal.wait_for(term, "never", 200)
    :ok = TuiLocal.kill(term)
    :ok = TuiLocal.close(term)
  end

  test "a missing terminal id is a :not_found error, not a crash" do
    assert {:error, %{variant: :not_found}} = TuiLocal.snapshot("nope")
  end
end
