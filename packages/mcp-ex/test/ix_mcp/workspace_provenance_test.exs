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

    {_d, quiet} = diagnostics("length(body)", "A uses it")
    refute quiet =~ "changed type under this workspace"
  end

  test "a same-typed takeover by a cell that ran alongside is reported as a note" do
    # `a` is still asleep when `b` binds `out`, so `a`'s own write lands
    # afterwards and takes the name from a cell it overlapped. That is the
    # collision this note exists for: two agents live at once.
    code = "Process.sleep(400); out = \"first\""
    {a, _} = Jobs.run(code, intent: "A binds out", budget: 0)
    {_b, _} = diagnostics(~s(out = "second"), "B binds out")

    summary = Jobs.await(a.id)
    warned = Enum.join(summary.diagnostics, "\n")

    assert warned =~ "note: shared binding: `out` was bound"
    assert warned =~ ~s(intent: "B binds out")
  end

  test "a same-typed rebind after the other cell finished says nothing" do
    # One agent reusing a scratch name across its own sequential cells. The
    # note here fired on nearly every cell and carried no information, so it
    # is suppressed; the type-change warning above is not.
    {_a, _} = diagnostics(~s(out = "first"), "A binds out")
    {_b, quiet} = diagnostics(~s(out = "second"), "B binds out")
    refute quiet =~ "shared binding"

    # Same type means no crash waiting downstream, so the reader is not
    # interrupted either.
    {_c, still_quiet} = diagnostics("String.length(out)", "A uses out")
    refute still_quiet =~ "shared binding"
  end

  test "a cell rebinding its own variable says nothing" do
    {_first, quiet} = diagnostics(~s(x = 1; x = x + 1; x), "one cell, two writes")
    refute quiet =~ "shared binding"
  end

  test "merely reading a variable does not take it over" do
    {a, _} = diagnostics(~s(shared = "value"), "A binds shared")
    {_b, quiet} = diagnostics("String.length(shared)", "B reads shared")
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

  test "a shadowed local is not a read of the shared variable" do
    {_a, _} = diagnostics(~s(body = "html"), "A binds body")
    {_b, _} = diagnostics(~s(body = ["lines"]), "B clobbers body")

    # `body` here is the anonymous function's own parameter and cannot reach
    # the workspace's, so warning about it would be crying wolf.
    {_c, quiet} = diagnostics("Enum.map([1, 2], fn body -> body + 1 end)", "C shadows body")
    refute quiet =~ "shared binding"

    {_d, quiet} = diagnostics("case {:ok, 1} do {:ok, body} -> body end", "C matches body")
    refute quiet =~ "shared binding"

    # Reading it and rebinding it in one expression is a real read.
    {_e, warned} = diagnostics("body = length(body)", "C reads then rebinds body")
    assert warned =~ "changed type under this workspace"
  end

  test "a cell that puts the type back is not reported as the one that broke it" do
    {_a, _} = diagnostics(~s(body = "html"), "A binds body")
    {_b, _} = diagnostics(~s(body = ["lines"]), "B clobbers body")
    {_c, _} = diagnostics(~s(body = "repaired"), "C repairs body")

    {_d, quiet} = diagnostics("String.length(body)", "A uses body again")
    refute quiet =~ "shared binding"
  end

  test "a defmodule the AST cannot resolve does not kill the cell" do
    {summary, _} =
      diagnostics(
        "defmodule OuterScan do\n  defmodule __MODULE__.Inner do\n    def hi, do: :ok\n  end\nend",
        "A defines a nested module"
      )

    assert summary.status == :done
    # The module is defined by the cell, so this file cannot call into it.
    assert function_exported?(OuterScan.Inner, :hi, 0)
  end

  test "a defmodule inside quote claims nothing" do
    {_a, _} =
      diagnostics("q = quote do\n  defmodule QuotedPage do\n  end\nend", "A quotes a module")

    {_b, quiet} =
      diagnostics("defmodule QuotedPage do\n  def render, do: :ok\nend", "B defines it for real")

    refute quiet =~ "shared module"
  end

  test "a failed cell claims neither its variables nor its modules" do
    {failed, _} =
      diagnostics(
        ~s(defmodule ClaimedOnFailure do\n  def hi, do: :ok\nend\n\nraise "boom"),
        "A defines then raises"
      )

    assert failed.status == :failed

    {_b, quiet} =
      diagnostics("defmodule ClaimedOnFailure do\n  def hi, do: :no\nend", "B defines it")

    refute quiet =~ "shared module"
  end

  test "an improper list does not take the workspace down" do
    workspace = Process.whereis(Workspace)
    {summary, _} = diagnostics(~s(iodata = ["a" | "b"]), "A binds iodata")

    assert summary.status == :done
    assert Process.whereis(Workspace) == workspace
    assert Enum.find(Workspace.owners(), &(&1.name == :iodata)).shape == "a term"
  end

  test "Ix.bindings reports names that predate the provenance map" do
    {_a, _} = diagnostics(~s(restored = "value"), "A binds restored")
    {binding, env} = Workspace.snapshot()
    IxMcp.Checkpoint.store(binding, env)
    IxMcp.Checkpoint.store_provenance(%{owners: %{}, contested: %{}, modules: %{}})

    :ok = Supervisor.terminate_child(IxMcp.Supervisor, Workspace)
    {:ok, _pid} = Supervisor.restart_child(IxMcp.Supervisor, Workspace)

    orphan = Enum.find(IxMcp.Kernel.bindings(), &(&1.name == :restored))
    assert orphan.job == nil
    assert orphan.shape == "a 5-byte binary"
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
