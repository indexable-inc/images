defmodule IxMcp.CmdTest do
  use ExUnit.Case, async: true

  alias IxMcp.Cmd

  # Everything here would hang forever under raw System.cmd/3: a port's
  # stdin pipe never closes, so a command that falls back to reading stdin
  # blocks until cancelled (#3867). The Task timeout turns "hangs forever"
  # into a test failure instead of a stuck suite.
  defp await_5s(fun) do
    fun |> Task.async() |> Task.await(5_000)
  end

  defp tmp_dir! do
    dir = Path.join(System.tmp_dir!(), "ix-cmd-test-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    on_exit(fn -> File.rm_rf!(dir) end)
    dir
  end

  test "a stdin-reading command sees EOF instead of hanging" do
    # `cat` with no file argument is the hermetic stand-in for pathless rg:
    # the identical read-stdin fallback, present in every environment.
    assert {"", 0} = await_5s(fn -> Cmd.run("cat") end)
  end

  # The literal #3867 repro; the sandboxed check env carries no ripgrep, so
  # this compiles away there and the `cat` test above covers the mechanism.
  if System.find_executable("rg") do
    test "pathless rg returns matches instead of hanging (#3867)" do
      dir = tmp_dir!()
      File.write!(Path.join(dir, "haystack.txt"), "needle\n")
      assert {out, 0} = await_5s(fn -> Cmd.run("rg", ["-n", "needle"], cd: dir) end)
      assert out =~ "needle"
    end
  end

  test "run passes System.cmd options through" do
    dir = tmp_dir!()
    File.write!(Path.join(dir, "marker"), "")
    assert {out, 0} = Cmd.run("ls", [], cd: dir)
    assert String.trim(out) == "marker"
  end

  test "sh redirects the whole script, pipeline heads included" do
    assert {out, 0} = await_5s(fn -> Cmd.sh("cat | wc -c") end)
    assert String.trim(out) == "0"
  end

  test "an explicit cd: naming a missing directory raises, not {\"\", 2} (#3979)" do
    gone = Path.join(System.tmp_dir!(), "ix-cmd-gone-#{System.unique_integer([:positive])}")

    assert_raise Cmd.DeadCwdError, ~r/is not a directory/, fn ->
      Cmd.run("echo", ["hi"], cd: gone)
    end
  end
end
