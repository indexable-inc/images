defmodule IxMcp.Stdlib.ShPreludeTest do
  use ExUnit.Case, async: false

  # Sh is only useful if a cell can say `Sh.` with no setup, and nothing else in
  # this suite would notice if it fell out of @prelude. The macros need the
  # prelude's `require` on top of the alias: without it `Sh.mutate ... do` parses
  # as a function call and the `verify` marker blows up as an undefined variable.

  test "Sh functions are reachable from a cell" do
    IxMcp.Workspace.reset()

    {summary, _out} =
      IxMcp.Jobs.run(~s|Sh.ok!(Sh.cmd(["printf", "%s", "hi"]))|, budget: 10, intent: "prelude")

    assert summary.status == :done
    assert summary.result == ~s|"hi"|
  end

  test "the mutate macro is usable from a cell, which takes `require`, not just `alias`" do
    IxMcp.Workspace.reset()

    marker =
      Path.join(System.tmp_dir!(), "sh-prelude-marker-#{System.unique_integer([:positive])}")

    on_exit(fn -> File.rm_rf(marker) end)

    code = """
    Sh.mutate "create the marker" do
      Sh.cmd(["touch", #{inspect(marker)}]) |> Sh.run()
    verify
      File.exists?(#{inspect(marker)})
    end
    """

    {summary, _out} = IxMcp.Jobs.run(code, budget: 15, intent: "prelude macro")

    assert summary.status == :done
    assert File.exists?(marker)
  end
end
