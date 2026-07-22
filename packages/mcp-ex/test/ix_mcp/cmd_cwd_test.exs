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
    assert {launch, 0} = Cmd.run("pwd")

    elsewhere = Path.join(System.tmp_dir!(), "ix-cwd-test-15235")
    File.mkdir_p!(elsewhere)
    on_exit(fn -> File.rm_rf!(elsewhere) end)

    before = File.cwd!()
    File.cd!(elsewhere)

    try do
      # Session A moved the OS cwd; session B's pathless commands must not
      # follow it -- run/3 and sh/2 both still answer with the launch dir.
      assert {^launch, 0} = Cmd.run("pwd")
      assert {^launch, 0} = Cmd.sh("pwd")

      # An explicit cd: still selects exactly the directory the caller names.
      assert {moved, 0} = Cmd.run("pwd", [], cd: elsewhere)
      assert moved != launch
      assert Path.basename(String.trim(moved)) == Path.basename(elsewhere)
    after
      File.cd!(before)
    end
  end

  # The #3979 incident: the directory the kernel booted in was deleted
  # mid-session, and from then on every Cmd.run/Cmd.sh -- `echo` included --
  # returned {"", 2} in ~0s with no diagnostics, while :os.cmd/1 in the same
  # cells kept working. The port child's failed chdir masquerades as the
  # command's own exit code; the wrapper must name the dead layer instead.
  test "a launch cwd deleted after boot raises a named error, not {\"\", 2} (#3979)" do
    live = Cmd.launch_cwd()
    before = File.cwd!()

    doomed = Path.join(System.tmp_dir!(), "ix-dead-launch-#{System.unique_integer([:positive])}")
    File.mkdir_p!(doomed)
    File.cd!(doomed)
    Cmd.capture_launch_cwd()
    File.cd!(before)

    on_exit(fn ->
      File.cd!(live)
      Cmd.capture_launch_cwd()
      File.cd!(before)
    end)

    File.rm_rf!(doomed)

    assert_raise Cmd.DeadCwdError, ~r/launch directory .* no longer exists/, fn ->
      Cmd.run("echo", ["hi"])
    end

    assert_raise Cmd.DeadCwdError, ~r/launch directory .* no longer exists/, fn ->
      Cmd.sh("echo hi")
    end
  end

  test "the launch cwd is captured at boot, before any cell can move it" do
    # Application.start/2 captured File.cwd!() before the supervision tree;
    # nothing in this suite has moved the OS cwd, so the two still agree.
    assert Cmd.launch_cwd() == File.cwd!()
  end
end
