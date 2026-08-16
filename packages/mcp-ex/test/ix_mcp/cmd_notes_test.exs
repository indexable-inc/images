defmodule IxMcp.CmdNotesTest do
  # "Inner command failure looked green" (error-report triage): a nonzero
  # subprocess exit inside a green cell now rides the job's diagnostics, and
  # Cmd.run!/sh! turn it into a failed cell outright.
  use ExUnit.Case, async: false

  alias IxMcp.Cmd
  alias IxMcp.Jobs

  setup do
    IxMcp.Workspace.reset()
    :ok
  end

  test "a nonzero exit inside a done cell lands as a note on the job" do
    {summary, _out} = Jobs.run(~s|Cmd.run("false")|, intent: "silent failure")
    assert summary.status == :done
    assert Enum.any?(summary.diagnostics, &(&1 =~ "`false` exited 1"))
  end

  test "a zero exit attaches nothing" do
    {summary, _out} = Jobs.run(~s|Cmd.run("true")|, intent: "clean run")
    assert summary.status == :done
    assert summary.diagnostics == []
  end

  test "Cmd.run! raises on nonzero, failing the cell with the status in the message" do
    {summary, _out} = Jobs.run(~s|Cmd.run!("false")|, intent: "loud failure")
    assert summary.status == :failed
    assert summary.result =~ "IxMcp.Cmd.Error"
    assert summary.result =~ "exited 1"
  end

  test "Cmd.run! returns output alone on success" do
    assert Cmd.run!("echo", ["ok"]) == "ok\n"
  end

  test "Cmd.sh! raises with the script's own diagnostic folded in" do
    error = assert_raise Cmd.Error, fn -> Cmd.sh!("echo doomed >&2; exit 3") end
    assert error.status == 3
    assert error.message =~ "doomed"
  end

  test "outside a cell a nonzero exit is silent (no job to note on)" do
    assert {_out, 1} = Cmd.run("false")
  end

  test "a quoted phrase inside ~w earns a scan-time hint" do
    {summary, _out} =
      Jobs.run(~S|args = ~w(rg "two words"); length(args)|, intent: "~w footgun")

    assert summary.status == :done
    assert Enum.any?(summary.diagnostics, &(&1 =~ "~w splits on whitespace only"))
  end

  test "a plain ~w earns no hint" do
    {summary, _out} = Jobs.run(~S|~w(a b c)|, intent: "plain ~w")
    assert summary.diagnostics == []
  end
end
