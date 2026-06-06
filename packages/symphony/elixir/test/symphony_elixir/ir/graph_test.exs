defmodule SymphonyElixir.IR.GraphTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.IR.{Graph, Node, RunGraph}

  defp node(id, opts) do
    Node.new(
      [id: id, ast_origin: {:test, id}, kind: :exec, inputs: Keyword.get(opts, :inputs, %{})] ++
        Keyword.take(opts, [:state])
    )
  end

  defp graph(nodes) do
    RunGraph.new("run-1", "hash", nil) |> RunGraph.put_nodes(nodes)
  end

  describe "ready_nodes/1" do
    test "a node with no deps is ready immediately" do
      g = graph([node("a", state: :pending)])
      assert [%Node{id: "a"}] = Graph.ready_nodes(g)
    end

    test "parallel-ready siblings are both returned" do
      g = graph([node("a", state: :pending), node("b", state: :pending)])
      ids = g |> Graph.ready_nodes() |> Enum.map(& &1.id)
      assert ids == ["a", "b"]
    end

    test "a dependent is not ready until its dep succeeds" do
      dep_input = %{"x" => {:node, "a", []}}
      g = graph([node("a", state: :pending), node("b", state: :pending, inputs: dep_input)])

      assert g |> Graph.ready_nodes() |> Enum.map(& &1.id) == ["a"]

      g = Graph.apply_output(g, "a", {:ok, %{result: 1}})
      assert g |> Graph.ready_nodes() |> Enum.map(& &1.id) == ["b"]
    end

    test "running and terminal nodes are excluded" do
      g =
        graph([
          node("a", state: :running),
          node("b", state: :succeeded),
          node("c", state: :pending)
        ])

      assert g |> Graph.ready_nodes() |> Enum.map(& &1.id) == ["c"]
    end

    test "fan-out: two independent dependents of one parent are both ready together" do
      inputs = %{"x" => {:node, "a", []}}

      g =
        graph([
          node("a", state: :succeeded),
          node("b", state: :pending, inputs: inputs),
          node("c", state: :pending, inputs: inputs)
        ])

      assert g |> Graph.ready_nodes() |> Enum.map(& &1.id) == ["b", "c"]
    end
  end

  describe "apply_output/3" do
    test "success marks the node succeeded and records output" do
      g = graph([node("a", state: :running)]) |> Graph.apply_output("a", {:ok, :done})
      assert g.nodes["a"].state == :succeeded
      assert g.nodes["a"].output == :done
    end

    test "failure propagates :upstream_failed to a waiting dependent" do
      inputs = %{"x" => {:node, "a", []}}
      g = graph([node("a", state: :running), node("b", state: :pending, inputs: inputs)])

      g = Graph.apply_output(g, "a", {:error, :boom})

      assert g.nodes["a"].state == :failed
      assert g.nodes["b"].state == :upstream_failed
    end

    test "failure propagates transitively through a chain" do
      g =
        graph([
          node("a", state: :running),
          node("b", state: :pending, inputs: %{"x" => {:node, "a", []}}),
          node("c", state: :pending, inputs: %{"y" => {:node, "b", []}})
        ])

      g = Graph.apply_output(g, "a", {:error, :boom})

      assert g.nodes["b"].state == :upstream_failed
      assert g.nodes["c"].state == :upstream_failed
    end

    test "a dependent that opts to run on failure is not propagated to" do
      inputs = %{"x" => {:node, "a", []}, "__on_failure__" => {:literal, true}}
      g = graph([node("a", state: :running), node("b", state: :pending, inputs: inputs)])

      g = Graph.apply_output(g, "a", {:error, :boom})

      assert g.nodes["b"].state == :pending
    end
  end

  describe "reset_node/2" do
    test "returns a terminal node to :pending and clears output" do
      g = graph([node("a", state: :failed)])
      g = %{g | nodes: %{"a" => %{g.nodes["a"] | output: {:error, :x}}}}

      g = Graph.reset_node(g, "a")

      assert g.nodes["a"].state == :pending
      assert g.nodes["a"].output == nil
    end
  end

  describe "finish detection" do
    test "all_terminal? is true only when every node is terminal" do
      refute Graph.all_terminal?(graph([node("a", state: :running)]))
      assert Graph.all_terminal?(graph([node("a", state: :succeeded), node("b", state: :skipped)]))
    end

    test "finished_status reflects failure and success" do
      assert Graph.finished_status(graph([node("a", state: :succeeded)])) == :succeeded
      assert Graph.finished_status(graph([node("a", state: :failed)])) == :failed
      assert Graph.finished_status(graph([node("a", state: :running)])) == :running
    end

    test "an empty node map is a no-op run that finishes succeeded" do
      # A workflow whose only construct gated its body off (`when` falsy,
      # `every n` that did not fire this tick) materializes to zero nodes.
      # That is a completed no-op, not a run still in progress, so the
      # runtime can finish it instead of tripping the deadlock guard.
      assert Graph.finished_status(graph([])) == :succeeded

      # The empty map stays non-terminal so the runtime never declares a run
      # done before its first materialization; the two invariants are the
      # deliberate pair the runtime relies on.
      refute Graph.all_terminal?(graph([]))
    end
  end
end
