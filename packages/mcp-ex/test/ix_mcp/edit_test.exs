defmodule IxMcp.EditTest do
  use ExUnit.Case, async: true

  alias IxMcp.Edit

  @moduletag :tmp_dir

  describe "replace/4" do
    test "replaces a unique match and returns a numbered snippet", %{tmp_dir: dir} do
      path = Path.join(dir, "unique.txt")
      File.write!(path, "alpha\nfoo bar\nomega\n")

      message = Edit.replace(path, "bar", "BAZ")

      assert File.read!(path) == "alpha\nfoo BAZ\nomega\n"
      assert message =~ "The file #{path} has been updated successfully."
      assert message =~ "     2\tfoo BAZ"
    end

    test "zero matches raise the not-found error", %{tmp_dir: dir} do
      path = Path.join(dir, "none.txt")
      File.write!(path, "alpha\n")

      assert_raise ArgumentError, ~r/String to replace not found in file/, fn ->
        Edit.replace(path, "zzz", "yyy")
      end
    end

    test "ambiguous matches raise unless replace_all", %{tmp_dir: dir} do
      path = Path.join(dir, "multi.txt")
      File.write!(path, "foo bar foo\n")

      assert_raise ArgumentError, ~r/Found 2 matches of the string to replace/, fn ->
        Edit.replace(path, "foo", "qux")
      end

      message = Edit.replace(path, "foo", "qux", replace_all: true)

      assert File.read!(path) == "qux bar qux\n"
      assert message =~ "All occurrences were successfully replaced."
    end

    test "identical old and new strings raise", %{tmp_dir: dir} do
      path = Path.join(dir, "same.txt")
      File.write!(path, "alpha\n")

      assert_raise ArgumentError, ~r/exactly the same/, fn ->
        Edit.replace(path, "alpha", "alpha")
      end
    end

    test "an empty old_string raises", %{tmp_dir: dir} do
      path = Path.join(dir, "empty-needle.txt")
      File.write!(path, "alpha\n")

      assert_raise ArgumentError, ~r/old_string must not be empty/, fn ->
        Edit.replace(path, "", "x")
      end
    end
  end

  describe "write/2" do
    test "creates with parents, then reports update on overwrite", %{tmp_dir: dir} do
      path = Path.join([dir, "nested", "deep", "new.txt"])

      assert Edit.write(path, "one") == "File created successfully at: #{path}"
      assert File.read!(path) == "one"

      assert Edit.write(path, "two") == "The file #{path} has been updated successfully."
      assert File.read!(path) == "two"
    end
  end

  describe "primary-checkout worktree guard (#3871)" do
    setup %{tmp_dir: dir} do
      primary = Path.join(dir, "primary")
      File.mkdir_p!(primary)
      git!(primary, ["init"])

      # git resolves symlinks (macOS /tmp -> /private/tmp), so globs must be
      # built from the toplevel git reports, not from the raw tmp_dir. `*`
      # crosses `/` in the hook's matcher, so one glob over the resolved tmp
      # root covers every repo this test creates under it.
      toplevel =
        primary
        |> git!(["rev-parse", "--path-format=absolute", "--show-toplevel"])
        |> String.trim()

      Application.put_env(:ix_mcp, :primary_checkouts, [Path.dirname(toplevel) <> "/*"])
      on_exit(fn -> Application.delete_env(:ix_mcp, :primary_checkouts) end)

      %{primary: toplevel}
    end

    test "write under a configured primary checkout raises and writes nothing", %{
      primary: primary
    } do
      target = Path.join(primary, "notes.txt")

      assert_raise ArgumentError, ~r/primary checkout, not a worktree/, fn ->
        Edit.write(target, "nope\n")
      end

      refute File.exists?(target)

      # A new file whose parents do not exist yet is judged by the nearest
      # existing ancestor, exactly like the hook.
      deep = Path.join([primary, "not", "yet", "there.txt"])

      assert_raise ArgumentError, ~r/use a worktree|worktree add/, fn ->
        Edit.write(deep, "nope\n")
      end

      refute File.exists?(deep)
    end

    test "replace under a configured primary checkout raises", %{primary: primary} do
      target = Path.join(primary, "existing.txt")
      File.write!(target, "alpha\n")

      assert_raise ArgumentError, ~r/primary checkout, not a worktree/, fn ->
        Edit.replace(target, "alpha", "beta")
      end

      assert File.read!(target) == "alpha\n"
    end

    test "a linked worktree of the protected checkout stays writable", %{
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

      wt = Path.join(dir, "wt")
      git!(primary, ["worktree", "add", wt])

      message = Edit.write(Path.join(wt, "notes.txt"), "fine\n")
      assert message =~ "File created successfully"
    end

    test "a matching path outside any git repo stays writable", %{tmp_dir: dir} do
      plain = Path.join(dir, "plain")
      File.mkdir_p!(plain)

      assert Edit.write(Path.join(plain, "notes.txt"), "fine\n") =~ "File created successfully"
    end
  end

  defp git!(dir, args) do
    {out, 0} = System.cmd("git", args, cd: dir, stderr_to_stdout: true)
    out
  end
end
