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

  # The #3979 incident: Erlang's child setup reports a failed chdir by
  # exiting with the raw errno, so a missing cd: came back as {"", 2} --
  # empty output, ENOENT dressed up as the command's own exit status.
  describe "missing cd target (#3979)" do
    test "run works while the cd exists and raises once it is deleted" do
      dir = tmp_dir!()
      assert {"a\n", 0} = Cmd.run("echo", ["a"], cd: dir)

      File.rm_rf!(dir)
      err = assert_raise(ArgumentError, fn -> Cmd.run("echo", ["a"], cd: dir) end)
      assert err.message == "cd target #{dir} does not exist"
    end

    test "sh raises on a missing cd instead of returning {\"\", 2}" do
      dir = tmp_dir!()
      assert {"a\n", 0} = Cmd.sh("echo a", cd: dir)

      File.rm_rf!(dir)
      assert_raise(ArgumentError, ~r/does not exist/, fn -> Cmd.sh("echo a", cd: dir) end)
    end

    test "a cd that is a file, not a directory, raises (was {\"\", 20})" do
      file = Path.join(tmp_dir!(), "plain-file")
      File.write!(file, "")

      assert_raise(ArgumentError, ~r/is not a directory/, fn ->
        Cmd.run("echo", ["a"], cd: file)
      end)
    end

    test "a cd deleted mid-command raises instead of returning an ambiguous status" do
      dir = tmp_dir!()

      # The command removes its own cwd and exits nonzero: the same
      # after-the-spawn shape as a validate/spawn race, deterministically.
      assert_raise(RuntimeError, ~r/no longer exists/, fn ->
        Cmd.sh(~S(rmdir "$PWD" && exit 3), cd: dir)
      end)
    end
  end
end
