defmodule IxMcp.ApiTest do
  use ExUnit.Case, async: false

  alias IxMcp.Jobs

  setup do
    IxMcp.Workspace.reset()
    :ok
  end

  test "api() lists the bundled surface with summaries, filterable" do
    rows = IxMcp.Api.api("tail")
    assert Enum.any?(rows, &(&1.name == :tail and &1.module == IxMcp.Jobs))
  end

  test "help/2 returns a function's full doc" do
    text = IxMcp.Api.help(IxMcp.Jobs, :tail)
    assert text =~ "tail("
    assert text =~ "Last"
  end

  test "the folded-in tool surface is aliased in cells" do
    path = Path.join(System.tmp_dir!(), "ix-mcp-read-test-#{System.unique_integer([:positive])}")
    File.write!(path, "one\ntwo\nthree\n")
    on_exit(fn -> File.rm(path) end)

    {summary, _} = Jobs.run(~s|Read.file("#{path}", 2, 2)|, intent: "Read from a cell")
    assert summary.status == :done
    assert summary.result == inspect("two")

    {aliases, _} = Jobs.run("{Ix, PrWatch, Tui}", intent: "aliases resolve")
    assert aliases.result == "{IxMcp.Kernel, IxMcp.PrWatch, IxMcp.Tui}"
  end

  test "Api is aliased in cells, like Jobs" do
    {summary, _} = Jobs.run("Api.api(\"grep\") |> length()", intent: "use Api from a cell")
    assert summary.status == :done
    assert summary.result >= "1"
  end
end
