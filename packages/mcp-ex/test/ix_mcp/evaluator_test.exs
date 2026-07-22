defmodule IxMcp.EvaluatorTest do
  use ExUnit.Case, async: false

  alias IxMcp.Jobs

  setup do
    IxMcp.Workspace.reset()
    :ok
  end

  test "bindings persist across cells" do
    {summary, _out} = Jobs.run("x = 41", intent: "bind x")
    assert summary.status == :done

    {summary, _out} = Jobs.run("x + 1", intent: "use x")
    assert summary.status == :done
    assert summary.result == "42"
  end

  test "aliases and modules defined in cells persist" do
    {summary, _out} =
      Jobs.run("defmodule CellHelper, do: def double(n), do: n * 2", intent: "define module")

    assert summary.status == :done

    {summary, _out} = Jobs.run("CellHelper.double(21)", intent: "call module")
    assert summary.status == :done
    assert summary.result == "42"
  end

  test "stdout is captured per job, including from spawned processes" do
    code = """
    IO.puts("from the cell")
    Task.await(Task.async(fn -> IO.puts("from a spawned task") end))
    """

    {summary, output} = Jobs.run(code, intent: "print")
    assert summary.status == :done
    assert output =~ "from the cell"
    assert output =~ "from a spawned task"
  end

  test "parse errors gate the cell: nothing evaluates" do
    {summary, _out} = Jobs.run("x = ) nonsense", intent: "syntax error")
    assert summary.status == :failed
    assert summary.result =~ "parse error"

    # The workspace must be untouched.
    refute :x in IxMcp.Workspace.names()
  end

  test "heredoc parse errors carry the exact hint; other parse errors do not" do
    hint = "hint: Elixir heredocs need a newline after the opening triple quote"

    {summary, _out} = Jobs.run(~s("""inline"""), intent: "inline heredoc")
    assert summary.status == :failed
    assert summary.result =~ "heredoc allows only whitespace"
    assert summary.result =~ hint

    {summary, _out} = Jobs.run(~s("""\nunterminated), intent: "unterminated heredoc")
    assert summary.status == :failed
    assert summary.result =~ "missing terminator"
    assert summary.result =~ hint

    # An ordinary syntax error and an unterminated plain string ("for string",
    # not "for heredoc") must stay hint-free.
    {summary, _out} = Jobs.run("x = ) nonsense", intent: "ordinary parse error")
    assert summary.status == :failed
    refute summary.result =~ "hint:"

    {summary, _out} = Jobs.run(~s("unterminated), intent: "unterminated string")
    assert summary.status == :failed
    assert summary.result =~ "missing terminator"
    refute summary.result =~ "hint:"
  end

  test "runtime errors report a formatted exception and keep prior bindings" do
    {_summary, _out} = Jobs.run("kept = :safe", intent: "bind")
    {summary, _out} = Jobs.run("1 / 0", intent: "raise")

    assert summary.status == :failed
    assert summary.result =~ "ArithmeticError"
    assert :kept in IxMcp.Workspace.names()
  end

  test "a failed cell merges nothing" do
    {summary, _out} = Jobs.run("doomed = 1; raise \"boom\"", intent: "fail late")
    assert summary.status == :failed
    refute :doomed in IxMcp.Workspace.names()
  end

  test "compiler warnings are surfaced as diagnostics without failing the cell" do
    {summary, _out} = Jobs.run("fn -> unused = 1 end.()", intent: "warn")
    assert summary.status == :done
    assert Enum.any?(summary.diagnostics, &(&1 =~ "unused"))
  end
end
