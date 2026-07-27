defmodule DashboardExTest do
  # Not async: documents live in one process-global registry and each test
  # binds a real loopback listener; keep them ordered.
  use ExUnit.Case, async: false

  alias DashboardEx.{DashboardError, DocEvent, Native}

  # What this suite proves, offline: a hub really serves, panes published
  # from the BEAM reach the shared document, a peer's CRDT bytes merge into
  # it, and the watch stream pushes `{:unibind_stream, ref, ...}` under
  # granted demand -- the wire contract ix-mcp-ex's bridge GenServer is
  # written against. No browser is involved; a browser's edit arrives as
  # `merge/2`, which is exactly what the merge tests exercise.

  setup context do
    doc = "test-#{:erlang.phash2(context.test)}"
    on_exit(fn -> DashboardEx.close(doc) end)
    {:ok, doc: doc}
  end

  test "open serves the document on loopback and close retires the id", %{doc: doc} do
    assert {:ok, url} = DashboardEx.open(doc)
    assert url =~ ~r{^http://127\.0\.0\.1:\d+}
    assert doc in DashboardEx.list()
    assert DashboardEx.url(doc) == {:ok, url}

    assert :ok = DashboardEx.close(doc)
    refute doc in DashboardEx.list()
    assert {:error, %DashboardError{variant: :not_found}} = DashboardEx.url(doc)
  end

  test "opening the same id twice is refused rather than orphaning a server", %{doc: doc} do
    assert {:ok, _url} = DashboardEx.open(doc)
    assert {:error, %DashboardError{variant: :already_open}} = DashboardEx.open(doc)
  end

  test "published panes reach the shared document and can be dropped", %{doc: doc} do
    {:ok, _url} = DashboardEx.open(doc)

    assert :ok = DashboardEx.set_html(doc, "notes", "Notes", "<b>hello-dashboard</b>")
    assert :ok = DashboardEx.set_data(doc, "metrics", "Metrics", "gauge", ~s({"cpu":0.5}))
    assert {:ok, panes} = DashboardEx.panes(doc)
    assert Enum.sort(panes) == ["metrics", "notes"]

    {:ok, json} = DashboardEx.value(doc)
    assert json =~ "hello-dashboard"
    assert json =~ "cpu"

    assert :ok = DashboardEx.drop_pane(doc, "metrics")
    assert DashboardEx.panes(doc) == {:ok, ["notes"]}
  end

  test "a peer's snapshot merges in, which is how a browser edit arrives", %{doc: doc} do
    peer = doc <> "-peer"
    on_exit(fn -> DashboardEx.close(peer) end)
    {:ok, _} = DashboardEx.open(doc)
    {:ok, _} = DashboardEx.open(peer)

    :ok = DashboardEx.set_html(peer, "remote", "Remote", "<i>peer-wrote-this</i>")
    assert {:ok, snapshot} = DashboardEx.snapshot(peer)
    # Loro bytes cross as a binary, not base64 in a string.
    assert is_binary(snapshot)
    refute String.valid?(snapshot)

    assert {:ok, "applied"} = DashboardEx.merge(doc, snapshot)
    {:ok, json} = DashboardEx.value(doc)
    assert json =~ "peer-wrote-this"
  end

  test "malformed inputs are typed errors, not crashes", %{doc: doc} do
    {:ok, _} = DashboardEx.open(doc)

    # Not a Loro payload at all: the CRDT layer rejects it rather than the
    # NIF crashing on a decode.
    assert {:error, %DashboardError{variant: :crdt}} = DashboardEx.merge(doc, <<0, 1, 2, 255>>)

    assert {:error, %DashboardError{variant: :bad_input}} =
             DashboardEx.set_data(doc, "d", "D", "gauge", "{not json")

    assert {:error, %DashboardError{variant: :not_found}} =
             DashboardEx.set_html("no-such-doc", "p", "P", "<b/>")
  end

  test "watch pushes {:unibind_stream, ref, _} only under granted demand", %{doc: doc} do
    {:ok, _} = DashboardEx.open(doc)

    # The generated wrapper blocks on a bare `receive`, so the raw NIF is the
    # only usable entry from a process that has other work -- the shape
    # ix-mcp-ex's bridge GenServer uses. `ref` correlates the messages;
    # `handle` must stay reachable, because collecting it aborts the producer.
    ref = make_ref()
    assert {:ok, handle} = Native.watch(ref, doc)

    :ok = DashboardEx.set_html(doc, "notes", "Notes", "<b>first-edit</b>")
    refute_receive {:unibind_stream, ^ref, _}, 100

    Native.unibind_demand(handle, 1)
    assert_receive {:unibind_stream, ^ref, {:item, %DocEvent{} = event}}, 2_000
    assert event.doc == doc
    assert event.root == "panes"
    assert event.kind in ["map", "text", "list", "tree", "unknown"]

    # One credit, one item: the rest of the same commit's diffs wait.
    refute_receive {:unibind_stream, ^ref, _}, 100

    Native.unibind_demand(handle, 1)
    assert_receive {:unibind_stream, ^ref, {:item, %DocEvent{}}}, 2_000

    # A second edit still flows on the same handle: proof the subscription
    # outlived the first delivery rather than being a one-shot, and the last
    # use of `handle`, which is what keeps the GC from aborting the producer
    # before the assertion above lands.
    Native.unibind_demand(handle, 4)
    :ok = DashboardEx.set_html(doc, "notes", "Notes", "<b>second-edit</b>")
    assert_receive {:unibind_stream, ^ref, {:item, %DocEvent{}}}, 2_000
  end

  test "a merge from a peer wakes the watcher, which is the human-edit path", %{doc: doc} do
    peer = doc <> "-peer"
    on_exit(fn -> DashboardEx.close(peer) end)
    {:ok, _} = DashboardEx.open(doc)
    {:ok, _} = DashboardEx.open(peer)
    :ok = DashboardEx.set_html(peer, "remote", "Remote", "<i>from-a-peer</i>")

    ref = make_ref()
    {:ok, handle} = Native.watch(ref, doc)
    Native.unibind_demand(handle, 8)

    {:ok, snapshot} = DashboardEx.snapshot(peer)
    assert {:ok, "applied"} = DashboardEx.merge(doc, snapshot)

    assert_receive {:unibind_stream, ^ref, {:item, %DocEvent{doc: ^doc}}}, 2_000
  end
end
