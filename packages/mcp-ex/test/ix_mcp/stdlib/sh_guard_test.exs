defmodule IxMcp.Stdlib.ShGuardTest do
  # async: false on purpose: these tests set :primary_checkouts in the global
  # application env, so running them beside another test that reads it would make
  # both flaky. The rest of the Sh suite stays async in sh_test.exs.
  use ExUnit.Case, async: false

  alias IxMcp.Stdlib.Sh

  @moduletag :tmp_dir

  setup %{tmp_dir: dir} do
    primary = Path.join(dir, "primary")
    File.mkdir_p!(primary)
    git!(primary, ["init"])

    # git resolves symlinks (macOS /tmp -> /private/tmp), so the globs come from
    # the toplevel git itself reports rather than from the raw tmp_dir.
    toplevel =
      primary
      |> git!(["rev-parse", "--path-format=absolute", "--show-toplevel"])
      |> String.trim()

    Application.put_env(:ix_mcp, :primary_checkouts, [Path.dirname(toplevel) <> "/*"])
    on_exit(fn -> Application.delete_env(:ix_mcp, :primary_checkouts) end)

    File.write!(Path.join(toplevel, "tracked.txt"), "alpha\n")

    %{primary: toplevel, scratch: dir}
  end

  defp git!(dir, args) do
    {out, 0} = System.cmd("git", args, cd: dir, stderr_to_stdout: true)
    out
  end

  defp staged(primary), do: git!(primary, ["diff", "--cached", "--name-only"])

  describe "the git guard reaches through Sh.run" do
    # THE CONTROLS, both directions. Before this file the guard on Sh's spawn path
    # had neither: `git`, `GitGuard` and `check_script` appeared nowhere in the Sh
    # suite, so a refusal that had quietly stopped working would have looked
    # exactly like a refusal that worked. A gate never shown able to say YES and to
    # say NO has not been validated, which is this module's own first law.
    test "positive control: a direct git mutation is refused and never runs", %{
      primary: primary,
      scratch: scratch
    } do
      assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
        Sh.run(Sh.cmd(["git", "add", "-A"], cd: primary), scratch_root: scratch)
      end

      assert staged(primary) == ""
    end

    test "negative control: a git READ in the same checkout is allowed", %{
      primary: primary,
      scratch: scratch
    } do
      result =
        Sh.run(Sh.cmd(["git", "status", "--porcelain"], cd: primary), scratch_root: scratch)

      assert Sh.ok?(result)
      assert result.out =~ "tracked.txt"
    end
  end

  describe "a shell's script argument is read, whatever the flag cluster spells" do
    # `sh -c "git add -A"` was closed; `sh -ec`, `bash -lc` and `sh -xc` were NOT,
    # because the seam matched an argv word EQUAL to "-c". All three of these
    # execute their operand as a script, so all three were a bypass of a guard
    # whose own comment claimed to have closed exactly this hole.
    for flag <- ["-c", "-ec", "-lc", "-xc"] do
      test "sh #{flag} <script> is refused", %{primary: primary, scratch: scratch} do
        assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
          Sh.run(Sh.cmd(["sh", unquote(flag), "cd #{primary} && git add -A"], cd: primary),
            scratch_root: scratch
          )
        end

        assert staged(primary) == ""
      end
    end

    test "the long --command spellings are refused", %{primary: primary, scratch: scratch} do
      assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
        Sh.run(Sh.cmd(["bash", "--command", "git add -A"], cd: primary), scratch_root: scratch)
      end

      assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
        Sh.run(Sh.cmd(["bash", "--command=git add -A"], cd: primary), scratch_root: scratch)
      end
    end

    test "a shell this fleet actually uses is covered too", %{primary: primary, scratch: scratch} do
      # `nu` is the interactive shell on these hosts and was missing from the
      # shell list. The refusal happens BEFORE the spawn, so this test does not
      # need nu installed -- which is also why its absence hid the gap.
      assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
        Sh.run(Sh.cmd(["nu", "-c", "git add -A"], cd: primary), scratch_root: scratch)
      end

      assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
        Sh.run(Sh.cmd(["fish", "-c", "git add -A"], cd: primary), scratch_root: scratch)
      end
    end

    test "a shell with no script operand is not refused", %{primary: primary, scratch: scratch} do
      # The negative control for the flag scan: `-c` as the LAST word has no script
      # after it, so there is nothing to read and nothing to refuse. Building the
      # step must not raise; a guard that invented an operand would refuse honest
      # calls, and asserting on the struct is a claim that can actually fail.
      assert %Sh.Step{} = Sh.cmd(["sh", "script.sh", "-c"], cd: primary)

      result = Sh.run(Sh.cmd(["sh", "-c", "echo fine"], cd: primary), scratch_root: scratch)
      assert Sh.ok?(result)
      assert result.out =~ "fine"
    end
  end

  describe "a wrapper does not hide the program behind it" do
    # A guard reading only argv[0] saw the wrapper. Each of these was measured as
    # a bypass: the wrapper execs its operand, so looking only at the first word
    # is looking at the wrong program.
    test "env does not hide git", %{primary: primary, scratch: scratch} do
      assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
        Sh.run(Sh.cmd(["env", "git", "add", "-A"], cd: primary), scratch_root: scratch)
      end

      assert staged(primary) == ""
    end

    test "a wrapper with its own operand does not hide git", %{primary: primary, scratch: scratch} do
      # `timeout 5 git add -A` and `nice -n 5 git push` are why the scan looks at
      # EVERY position rather than at "the first word that is not a flag": that
      # heuristic picks the number and lets git straight through.
      assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
        Sh.run(Sh.cmd(["timeout", "5", "git", "add", "-A"], cd: primary), scratch_root: scratch)
      end

      assert_raise ArgumentError, ~r/Refusing `git commit`/, fn ->
        Sh.run(Sh.cmd(["nice", "-n", "5", "git", "commit", "-m", "x"], cd: primary),
          scratch_root: scratch
        )
      end
    end

    test "a wrapper in front of a shell still gets the script read", %{
      primary: primary,
      scratch: scratch
    } do
      assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
        Sh.run(Sh.cmd(["env", "sh", "-ec", "git add -A"], cd: primary), scratch_root: scratch)
      end
    end

    test "a wrapper around something harmless still runs", %{scratch: scratch} do
      # The negative control for the wrapper scan: looking through a wrapper must
      # not turn every wrapped command into a refusal.
      result = Sh.run(Sh.cmd(["env", "echo", "hello"]), scratch_root: scratch)

      assert Sh.ok?(result)
      assert result.out =~ "hello"
    end
  end
end
