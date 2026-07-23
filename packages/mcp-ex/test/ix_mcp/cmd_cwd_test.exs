defmodule IxMcp.CmdCwdTest do
  # async: false -- these tests move the BEAM-global OS cwd, which would
  # race any concurrent test that resolves a relative path.
  use ExUnit.Case, async: false

  alias IxMcp.Cmd

  # The #3902 incident: several agent sessions shared one kernel, one
  # session File.cd!'d around its /tmp worktree, and a sibling's pathless
  # `git reset --hard` resolved against that shared OS cwd and wiped the
  # worktree. The fix removes the OS cwd from command spawning entirely:
  # pathless commands run in the boot-time launch directory, immutably.
  test "File.cd! cannot redirect pathless commands off the launch cwd (#3902)" do
    # Every pwd here runs with -P: pwd defaults to -L and echoes the caller's
    # $PWD spelling (/tmp/... in a checkout under macOS's /tmp ->
    # /private/tmp symlink), while the launch cwd is captured
    # symlink-resolved via File.cwd!(), so only physical paths compare
    # byte-equal on both sides.
    assert {launch, 0} = Cmd.run("pwd", ["-P"])

    elsewhere = Path.join(System.tmp_dir!(), "ix-cwd-test-15235")
    File.mkdir_p!(elsewhere)
    on_exit(fn -> File.rm_rf!(elsewhere) end)

    before = File.cwd!()
    File.cd!(elsewhere)

    try do
      # Session A moved the OS cwd; session B's pathless commands must not
      # follow it -- run/3 and sh/2 both still answer with the launch dir.
      assert {^launch, 0} = Cmd.run("pwd", ["-P"])
      assert {^launch, 0} = Cmd.sh("pwd -P")

      # An explicit cd: still selects exactly the directory the caller names.
      assert {moved, 0} = Cmd.run("pwd", ["-P"], cd: elsewhere)
      assert moved != launch
      assert Path.basename(String.trim(moved)) == Path.basename(elsewhere)
    after
      File.cd!(before)
    end
  end

  test "the launch cwd is captured at boot, before any cell can move it" do
    # Application.start/2 captured File.cwd!() before the supervision tree;
    # nothing in this suite has moved the OS cwd, so the two still agree.
    assert Cmd.launch_cwd() == File.cwd!()
  end

  # The #3979 incident shape: a session launched from a directory that is
  # later deleted (a cleaned-up git worktree) saw every pathless command
  # return {"", 2} forever. The default-cd path must instead raise and
  # point at the launch dir.
  test "a deleted launch dir raises with the launch-dir hint (#3979)" do
    doomed = Path.join(System.tmp_dir!(), "ix-cwd-doomed-#{System.unique_integer([:positive])}")
    File.mkdir_p!(doomed)

    before = File.cwd!()
    File.cd!(doomed)
    Cmd.capture_launch_cwd()
    File.cd!(before)
    # File.cwd!() resolves symlinks (/tmp -> /private/tmp on macOS), so the
    # captured path is the canonical spelling, not `doomed` verbatim.
    captured = Cmd.launch_cwd()

    try do
      assert {_out, 0} = Cmd.run("pwd")

      File.rm_rf!(doomed)
      err = assert_raise(ArgumentError, fn -> Cmd.run("pwd") end)
      assert err.message == "cd target #{captured} does not exist (session launch dir deleted?)"
      assert_raise(ArgumentError, ~r/launch dir deleted/, fn -> Cmd.sh("pwd") end)
    after
      # Recapture the real launch dir so no later test inherits the stub.
      Cmd.capture_launch_cwd()
      File.rm_rf!(doomed)
    end
  end
end
