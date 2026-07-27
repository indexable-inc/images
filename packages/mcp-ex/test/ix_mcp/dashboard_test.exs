defmodule IxMcp.DashboardTest do
  # Not async: documents live in a process-global registry inside the NIF and
  # the bridge is a singleton whose viewer set is shared state.
  use ExUnit.Case, async: false

  import IxMcpTest.Eventually

  alias IxMcp.ActionLog
  alias IxMcp.Dashboard
  alias IxMcp.Session

  @moduletag :dashboard_ex

  # What these defend: the runtime load of the :dashboard_ex app (code path,
  # app load, NIF @on_load), the facade's argument plumbing, and the piece
  # that only exists here -- the bridge turning a document change into a
  # durable, per-viewer outbox row. The CRDT behaviour itself is covered by
  # the binding's own suite (checks.*.dashboard-ex-run).

  setup context do
    doc = "mcp-test-#{:erlang.phash2(context.test)}"
    on_exit(fn -> Dashboard.close(doc) end)
    {:ok, doc: doc}
  end

  test "loads the NIF app and reports an unknown document as a typed error" do
    assert {:error, %{variant: :not_found}} = Dashboard.url("no-such-document")
  end

  test "panes published from a cell land in the served document", %{doc: doc} do
    assert {:ok, url} = Dashboard.open(doc)
    assert url =~ "http://127.0.0.1:"

    assert :ok = Dashboard.html(doc, "notes", "Notes", "<b>from-a-cell</b>")
    assert :ok = Dashboard.data(doc, "stats", "Stats", "gauge", %{files: 12})

    assert {:ok, value} = Dashboard.value(doc)
    encoded = JSON.encode!(value)
    assert encoded =~ "from-a-cell"
    assert encoded =~ "files"
  end

  test "a viewing session gets a durable outbox row per edit window", %{doc: doc} do
    %{session_id: session} = Session.ids()
    {:ok, _url} = Dashboard.open(doc)

    assert :ok = Dashboard.view(doc)
    assert session in Dashboard.viewers(doc)

    # A publish from this kernel is used as the change here because the
    # binding reports every change without saying who made it (no public
    # dashboard-core signal carries the author yet). A browser's edit takes
    # the identical path -- `Hub::import` -> mirror -> watch stream.
    :ok = Dashboard.html(doc, "notes", "Notes", "<b>an-edit</b>")

    # The row is what survives a disconnected viewer, so assert on the
    # ledger rather than on a delivered message: with no transport
    # registered nothing is delivered and nothing is acked.
    row =
      eventually(
        fn -> Enum.find(ActionLog.unacked_outbox(session), &dashboard_row?(&1, doc)) end,
        # Twice the default window: this row only lands after the NIF's watch
        # stream has pushed and the bridge has written it.
        100
      )

    assert row.status == :done
    assert row.session_id == session

    assert :ok = Dashboard.unview(doc)
    refute session in Dashboard.viewers(doc)
  end

  defp dashboard_row?(row, doc) do
    is_binary(row.job_id) and String.starts_with?(row.job_id, "dashboard-#{doc}-")
  end
end
