defmodule IxMcp.WorkspaceProvenanceTest do
  @moduledoc """
  The shared-workspace collision guard (#3967). One kernel is one MCP
  connection, and a Claude Code subagent rides its parent's connection, so
  the parent's cells and the subagent's cells are different jobs on one
  binding map. These tests drive that exact shape: cell A binds `body` to a
  string, cell B binds it to a list of lines, cell A uses `body`.
  """

  use ExUnit.Case, async: false

  alias IxMcp.Jobs
  alias IxMcp.Workspace

  setup do
    Workspace.reset()
    :ok
  end

  defp diagnostics(code, intent) do
    {summary, _output} = Jobs.run(code, intent: intent)
    {summary, Enum.join(summary.diagnostics, "\n")}
  end

  test "a cell taking over another cell's variable is told what it replaced" do
    {_a, _quiet} = diagnostics(~s(body = "<html>dashboard</html>"), "agent A: render the body")

    {b, warned} =
      diagnostics(~s(body = ["line one", "line two"]), "agent B: read a nix file into lines")

    assert b.status == :done
    assert warned =~ "warning: shared binding: `body` was bound"
    assert warned =~ ~s(intent: "agent A: render the body")
    assert warned =~ "as a 22-byte binary"
    assert warned =~ "rebinds it as a 2-element list"
  end

  test "the cell that reads the clobbered variable is told before it uses it" do
    {a, _quiet} = diagnostics(~s(body = "<html>dashboard</html>"), "agent A: render the body")
    {_b, _warned} = diagnostics(~s(body = ["line one"]), "agent B: read a nix file into lines")

    {victim, warned} = diagnostics(~s(inflight = "<p>x</p>" <> body), "agent A: write the panels")

    # The read side has to be reported by a cell that raises, because that is
    # the cell the incident actually happened in: `<>` on a list of lines.
    assert victim.status == :failed
    assert victim.result =~ "not a bitstring"
    assert warned =~ "warning: shared binding: `body` changed type under this workspace"
    assert warned =~ ~s(intent: "agent B: read a nix file into lines")
    assert warned =~ "from a 22-byte binary to a 1-element list"
    assert warned =~ ~s(intent: "agent A: render the body")
    refute warned =~ "job #{a.id} (intent: \"agent A: render the body\") rebound"
  end

  test "the read-side warning stops once somebody binds the name again" do
    {_a, _} = diagnostics(~s(body = "html"), "A binds")
    {_b, _} = diagnostics(~s(body = ["lines"]), "B clobbers")
    {_c, warned} = diagnostics(~s(body = ["settled"]), "A takes it back")
    assert warned =~ "changed type under this workspace"

    {_d, quiet} = diagnostics(~s[length(body)], "A uses it")
    refute quiet =~ "changed type under this workspace"
  end

  test "a same-typed takeover is reported as a note, not a type-change warning" do
    {_a, _} = diagnostics(~s(out = "first"), "A binds out")
    {_b, warned} = diagnostics(~s(out = "second"), "B binds out")

    assert warned =~ "note: shared binding: `out` was bound"
    assert warned =~ ~s(intent: "A binds out")

    # Same type means no crash waiting downstream, so the reader is not
    # interrupted; the write side is where the collision is visible.
    {_c, quiet} = diagnostics(~s[String.length(out)], "A uses out")
    refute quiet =~ "shared binding"
  end

  test "a cell rebinding its own variable says nothing" do
    {_first, quiet} = diagnostics(~s(x = 1; x = x + 1; x), "one cell, two writes")
    refute quiet =~ "shared binding"
  end

  test "merely reading a variable does not take it over" do
    {a, _} = diagnostics(~s(shared = "value"), "A binds shared")
    {_b, quiet} = diagnostics(~s[String.length(shared)], "B reads shared")
    refute quiet =~ "shared binding"

    owner = Enum.find(Workspace.owners(), &(&1.name == :shared))
    assert owner.job == a.id
    assert owner.intent == "A binds shared"
  end

  test "Ix.bindings names who bound what" do
    {a, _} = diagnostics(~s(body = "<html/>"), "A renders")
    owner = Enum.find(IxMcp.Kernel.bindings(), &(&1.name == :body))

    assert owner.job == a.id
    assert owner.intent == "A renders"
    assert owner.shape == "a 7-byte binary"
  end

  test "redefining another cell's module names the cell that had it" do
    {a, _} =
      diagnostics(
        ~s(defmodule ProvenancePage, do: def render, do: "one"),
        "A defines ProvenancePage"
      )

    {b, warned} =
      diagnostics(
        ~s(defmodule ProvenancePage, do: def render, do: "two"),
        "B defines ProvenancePage"
      )

    assert b.status == :done
    assert warned =~ "warning: shared module: ProvenancePage was defined"
    assert warned =~ "by job #{a.id}"
    assert warned =~ ~s(intent: "A defines ProvenancePage")
    assert warned =~ "redefines it for every agent on this kernel"

    # The compiler's own notice still fires; ours adds the owner it omits.
    assert warned =~ "redefining module ProvenancePage"
  end

  test "a cell redefining its own module says nothing new" do
    {_a, quiet} =
      diagnostics(
        ~s(defmodule OwnPage, do: def render, do: "one"),
        "A defines OwnPage"
      )

    refute quiet =~ "shared module"
  end

  test "provenance survives a workspace restart" do
    {a, _} = diagnostics(~s(kept = "value"), "A binds kept")

    :ok = Supervisor.terminate_child(IxMcp.Supervisor, Workspace)
    {:ok, _pid} = Supervisor.restart_child(IxMcp.Supervisor, Workspace)

    owner = Enum.find(Workspace.owners(), &(&1.name == :kept))
    assert owner.job == a.id
    assert owner.intent == "A binds kept"
  end
end
