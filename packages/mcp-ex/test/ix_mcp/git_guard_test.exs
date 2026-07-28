defmodule IxMcp.GitGuardTest do
  use ExUnit.Case, async: false

  alias IxMcp.Cmd

  @moduletag :tmp_dir

  setup %{tmp_dir: dir} do
    primary = Path.join(dir, "primary")
    File.mkdir_p!(primary)
    git!(primary, ["init"])

    # git resolves symlinks (macOS /tmp -> /private/tmp), so the globs are
    # built from the toplevel git reports rather than the raw tmp_dir. `*`
    # crosses `/` in the hook's matcher, so one glob covers every repo this
    # test creates under the resolved root.
    toplevel =
      primary
      |> git!(["rev-parse", "--path-format=absolute", "--show-toplevel"])
      |> String.trim()

    Application.put_env(:ix_mcp, :primary_checkouts, [Path.dirname(toplevel) <> "/*"])
    on_exit(fn -> Application.delete_env(:ix_mcp, :primary_checkouts) end)

    File.write!(Path.join(toplevel, "tracked.txt"), "alpha\n")

    %{primary: toplevel}
  end

  test "a mutating git command in a protected checkout is refused and never runs", %{
    primary: primary
  } do
    assert_raise ArgumentError, ~r/Refusing `git add` in #{primary}/, fn ->
      Cmd.run("git", ["add", "-A"], cd: primary)
    end

    assert git!(primary, ["diff", "--cached", "--name-only"]) == ""
  end

  test "the refusal names the escape hatch and the kill switch", %{primary: primary} do
    message =
      assert_raise ArgumentError, fn -> Cmd.run("git", ["commit", "-m", "x"], cd: primary) end

    assert message.message =~ "worktree add /tmp/worktree/<org>/<repo>/<name>"
    assert message.message =~ "CLAUDE_CODE_DISABLE_GIT_GUARD=1"
  end

  test "reads in the same checkout stay allowed", %{primary: primary} do
    assert {_out, 0} = Cmd.run("git", ["status", "--porcelain"], cd: primary)
    assert {_out, 0} = Cmd.run("git", ["ls-files"], cd: primary)
    assert {_out, 0} = Cmd.run("git", ["stash", "list"], cd: primary)
    assert {_out, 0} = Cmd.run("git", ["add", "--dry-run", "."], cd: primary)
  end

  test "a subcommand outside the closed set fails open", %{primary: primary} do
    # `notes` mutates, and is deliberately unclassified: an unknown
    # subcommand must not become a refusal the first time someone runs it.
    assert {_out, _status} = Cmd.run("git", ["notes", "list"], cd: primary)
  end

  test "`git -C` aims the guard at the target, not the caller's cwd", %{
    tmp_dir: dir,
    primary: primary
  } do
    outside = Path.join(dir, "outside")
    File.mkdir_p!(outside)

    assert_raise ArgumentError, ~r/Refusing `git add`/, fn ->
      Cmd.run("git", ["-C", primary, "add", "-A"], cd: outside)
    end
  end

  test "a shell script is judged per command position", %{primary: primary} do
    assert_raise ArgumentError, ~r/Refusing `git switch`/, fn ->
      Cmd.sh("git fetch --quiet && git switch -c scratch", cd: primary)
    end

    # Non-git work in the same script is untouched.
    assert {"ok\n", 0} = Cmd.sh("echo ok", cd: primary)
  end

  test "a linked worktree of the protected checkout is not protected", %{
    tmp_dir: dir,
    primary: primary
  } do
    git!(primary, [
      "-c",
      "user.name=t",
      "-c",
      "user.email=t@t.invalid",
      "commit",
      "--allow-empty",
      "-m",
      "init"
    ])

    linked = Path.join(dir, "linked")
    git!(primary, ["worktree", "add", linked])

    File.write!(Path.join(linked, "new.txt"), "fine\n")
    assert {_out, 0} = Cmd.run("git", ["add", "-A"], cd: linked)
  end

  test "the kill switch allows the same command", %{primary: primary} do
    System.put_env("CLAUDE_CODE_DISABLE_GIT_GUARD", "1")
    on_exit(fn -> System.delete_env("CLAUDE_CODE_DISABLE_GIT_GUARD") end)

    assert {_out, 0} = Cmd.run("git", ["add", "-A"], cd: primary)
  end

  test "a repo no glob matches is untouched", %{tmp_dir: dir} do
    other = Path.join(dir, "elsewhere")
    File.mkdir_p!(other)
    git!(other, ["init"])
    Application.put_env(:ix_mcp, :primary_checkouts, ["/nowhere/*"])

    File.write!(Path.join(other, "f.txt"), "x\n")
    assert {_out, 0} = Cmd.run("git", ["add", "-A"], cd: other)
  end

  # System.cmd, not Cmd.run: the fixtures must not be judged by the guard
  # they are setting up.
  defp git!(dir, args) do
    {out, 0} = System.cmd("git", args, cd: dir, stderr_to_stdout: true)
    out
  end
end
