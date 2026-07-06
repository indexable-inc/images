defmodule SymphonyElixir.IR.RecoveryTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.IR.Attempt
  alias SymphonyElixir.IR.Graph
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.Runtime.Recovery

  defp node(id, opts) do
    Node.new(
      [id: id, ast_origin: {:test, id}, kind: :agent, inputs: Keyword.get(opts, :inputs, %{})] ++
        Keyword.take(opts, [:state, :attempts])
    )
  end

  defp running_with_thread(id, thread_id, opts \\ []) do
    attempt = Attempt.start(1, :codex, thread_id)
    node(id, Keyword.merge([state: :running, attempts: [attempt]], opts))
  end

  defp graph(nodes), do: "r" |> RunGraph.new("h", {:ast, []}) |> RunGraph.put_nodes(nodes)

  describe "replay/2" do
    test "replaying an expansion log reproduces the same node set deterministically" do
      base = graph([node("root", state: :succeeded)])

      log = RunGraph.append_expansion(base, {:fanout, "f"}, [:a, :b], ["child-a", "child-b"])

      expand = fn {:fanout, "f"}, elements, _nodes ->
        Enum.map(elements, fn e -> node("child-#{e}", state: :pending) end)
      end

      one = Recovery.replay(log, expand)
      two = Recovery.replay(log, expand)

      assert one.nodes |> Map.keys() |> Enum.sort() == ["child-a", "child-b", "root"]
      assert Map.keys(one.nodes) == Map.keys(two.nodes)
    end

    test "the default expander leaves a statically-materialized graph unchanged" do
      g = graph([node("a", state: :pending), node("b", state: :pending)])
      assert Recovery.replay(g).nodes == g.nodes
    end
  end

  describe "reconcile/2 reattach probe" do
    test "a :running node the engine still owns is left running" do
      g = graph([running_with_thread("a", "t1")])
      out = Recovery.reconcile(g, fn "t1" -> :running end)
      assert out.nodes["a"].state == :running
    end

    test "a :running node the engine finished is harvested" do
      g = graph([running_with_thread("a", "t1")])
      out = Recovery.reconcile(g, fn "t1" -> {:finished, {:ok, :harvested}} end)
      assert out.nodes["a"].state == :succeeded
      assert out.nodes["a"].output == :harvested
    end
  end

  describe "reconcile/2 strand policy (#90, non-idempotent safety)" do
    test "an unknown thread with an opened thread_id is stranded, never auto-retried" do
      # opted in but the attempt recorded a thread_id, so a side effect may
      # have happened: route to human review, do not blind-retry.
      g = graph([running_with_thread("a", "t1", inputs: %{"__retry__" => {:literal, true}})])
      out = Recovery.reconcile(g, fn "t1" -> :unknown end)
      assert out.nodes["a"].state == :stranded
    end

    test "an opted-in node with no observed side effect is auto-retried" do
      attempt = Attempt.start(1, :codex, nil)

      g =
        graph([
          node("a",
            state: :running,
            attempts: [attempt],
            inputs: %{"__retry__" => {:literal, true}}
          )
        ])

      out = Recovery.reconcile(g, fn nil -> :unknown end)
      assert out.nodes["a"].state == :retrying
    end

    test "a node that did not opt in is stranded even with no side effect" do
      attempt = Attempt.start(1, :codex, nil)
      g = graph([node("a", state: :running, attempts: [attempt])])
      out = Recovery.reconcile(g, fn nil -> :unknown end)
      assert out.nodes["a"].state == :stranded
    end

    test "the stranded attempt is recorded on the node" do
      g = graph([running_with_thread("a", "t1")])
      out = Recovery.reconcile(g, fn "t1" -> :unknown end)
      [att] = out.nodes["a"].attempts
      assert att.state == :stranded
      assert att.outcome == :stranded
    end

    test "after reconcile no node remains :running" do
      g = graph([running_with_thread("a", "t1"), running_with_thread("b", "t2")])
      out = Recovery.reconcile(g, fn _ -> :unknown end)
      refute Enum.any?(Graph.running_nodes(out))
    end
  end
end
