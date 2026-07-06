defmodule SymphonyElixirWeb.WorkflowsLiveTest do
  @moduledoc """
  Phase 6 tests: the :index page lists loaded fixture workflows and surfaces
  broken-.sym diagnostics; the :show page renders the graph SVG for a named
  workflow.
  """

  use ExUnit.Case, async: false

  import Phoenix.ConnTest
  import Phoenix.LiveViewTest

  alias SymphonyElixir.DSL.Parser

  @endpoint SymphonyElixirWeb.Endpoint

  # Each test seeds the ETS tables WorkflowCatalog exposes so we can test the
  # LiveView in isolation without starting the GenServer or touching the disk.
  setup do
    for table <- [:symphony_workflows, :symphony_workflow_errors] do
      if :ets.whereis(table) == :undefined do
        :ets.new(table, [:named_table, :public, read_concurrency: true])
      else
        :ets.delete_all_objects(table)
      end
    end

    :ok
  end

  # Minimal valid .sym source for a manual workflow with one exec node.
  @simple_sym ~s|workflow "inspect" on manual { a <- exec "./run.sh" }|

  defp insert_workflow(name, sym_source) do
    raw = sym_source
    hash = :crypto.hash(:sha256, raw)
    {:ok, ast} = Parser.parse(raw, file: "#{name}.sym")
    entry = %{name: name, ast: ast, trigger: ast.trigger, source: raw, hash: hash}
    :ets.insert(:symphony_workflows, {name, entry})
    entry
  end

  defp insert_error(name) do
    err = %{name: name, message: "unexpected token", line: 2, column: 5, file: "#{name}.sym"}
    :ets.insert(:symphony_workflow_errors, {name, err})
    err
  end

  test "index page lists a loaded workflow by name and trigger" do
    insert_workflow("inspect", @simple_sym)

    {:ok, _view, html} = live(build_conn(), "/workflows")

    assert html =~ "inspect"
    assert html =~ "manual"
  end

  test "index page links each workflow to its show page" do
    insert_workflow("inspect", @simple_sym)

    {:ok, _view, html} = live(build_conn(), "/workflows")

    assert html =~ ~s|href="/workflows/inspect"|
  end

  test "index page shows the empty state when no workflows are loaded" do
    {:ok, _view, html} = live(build_conn(), "/workflows")

    assert html =~ "no workflows loaded"
  end

  test "index page shows broken workflows panel when parse errors are present" do
    insert_workflow("inspect", @simple_sym)
    insert_error("broken")

    {:ok, _view, html} = live(build_conn(), "/workflows")

    assert html =~ "broken workflows"
    assert html =~ "broken.sym"
    assert html =~ "parse error"
    assert html =~ "unexpected token"
    # The healthy workflow must still appear.
    assert html =~ "inspect"
  end

  test "index page does not show broken panel when there are no errors" do
    insert_workflow("inspect", @simple_sym)

    {:ok, _view, html} = live(build_conn(), "/workflows")

    refute html =~ "broken workflows"
  end

  test "show page renders the graph svg for a loaded workflow" do
    insert_workflow("inspect", @simple_sym)

    {:ok, _view, html} = live(build_conn(), "/workflows/inspect")

    assert html =~ "<svg"
    assert html =~ "IR graph"
    # The exec node id from the workflow appears in the SVG.
    assert html =~ "inspect"
  end

  test "show page renders not-found message for an unknown workflow" do
    {:ok, _view, html} = live(build_conn(), "/workflows/no-such-workflow")

    assert html =~ "no workflow named"
    assert html =~ "no-such-workflow"
    assert html =~ "back to workflows"
  end

  test "workflows tab is active on index and show pages" do
    insert_workflow("inspect", @simple_sym)

    {:ok, _view, index_html} = live(build_conn(), "/workflows")
    assert index_html =~ ~s|class="active"|
    assert index_html =~ ~s|href="/workflows"|

    {:ok, _view, show_html} = live(build_conn(), "/workflows/inspect")
    assert show_html =~ ~s|class="active"|
    assert show_html =~ ~s|href="/workflows"|
  end
end
