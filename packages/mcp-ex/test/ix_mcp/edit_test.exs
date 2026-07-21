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
end
