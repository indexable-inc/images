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

  test "Api is aliased in cells, like Jobs" do
    {summary, _} = Jobs.run("Api.api(\"grep\") |> length()", intent: "use Api from a cell")
    assert summary.status == :done
    assert summary.result >= "1"
  end
end
