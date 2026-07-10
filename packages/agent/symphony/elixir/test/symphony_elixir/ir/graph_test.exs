defmodule SymphonyElixir.IR.GraphTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.IR.Attempt
  alias SymphonyElixir.IR.Graph
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph

  defp node(id, opts) do
    Node.new(
      [id: id, ast_origin: {:test, id}, kind: :exec, inputs: Keyword.get(opts, :inputs, %{})] ++
        Keyword.take(opts, [:state])
    )
  end

  defp graph(nodes) do
    "run-1" |> RunGraph.new("hash", nil) |> RunGraph.put_nodes(nodes)
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
      g = [node("a", state: :running)] |> graph() |> Graph.apply_output("a", {:ok, :done})
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

  describe "attempt bookkeeping" do
    test "mark_running opens a running attempt recording the executor kind" do
      g = graph([node("a", state: :pending)])

      g = Graph.mark_running(g, g.nodes["a"], 1)

      assert %Node{state: :running, attempts: [attempt]} = g.nodes["a"]
      assert %Attempt{n: 1, engine: :exec, state: :running, outcome: nil} = attempt
    end

    test "running -> finished: the open attempt closes with outcome, cost, and thread id" do
      cost = %{usd: 0.5, tokens_in: 10}
      g = graph([node("a", state: :pending)])

      g = Graph.mark_running(g, g.nodes["a"], 1)
      g = Graph.record_finished_attempt(g, "a", {:ok, %{cost: cost}}, "thread-1")

      assert [attempt] = g.nodes["a"].attempts
      assert %Attempt{n: 1, state: :succeeded, outcome: :ok, cost: ^cost, thread_id: "thread-1"} = attempt
      assert attempt.finished_at
    end

    test "a failed result closes the attempt :failed with the error and no cost" do
      g = graph([node("a", state: :pending)])

      g = Graph.mark_running(g, g.nodes["a"], 1)
      g = Graph.record_finished_attempt(g, "a", {:error, :boom}, nil)

      assert [%Attempt{state: :failed, outcome: {:error, :boom}, cost: nil}] = g.nodes["a"].attempts
    end

    test "cost accumulates per attempt: a retry's cost lands on the new attempt only" do
      g = graph([node("a", state: :pending)])

      g = Graph.mark_running(g, g.nodes["a"], 1)
      g = Graph.record_finished_attempt(g, "a", {:ok, %{cost: %{usd: 1.0}}}, "t1")
      g = Graph.mark_running(g, g.nodes["a"], 2)
      g = Graph.record_finished_attempt(g, "a", {:ok, %{cost: %{usd: 2.0}}}, "t2")

      assert [%Attempt{n: 1, cost: %{usd: 1.0}}, %Attempt{n: 2, cost: %{usd: 2.0}}] =
               Enum.sort_by(g.nodes["a"].attempts, & &1.n)
    end

    test "record_finished_attempt synthesizes an attempt when none was recorded" do
      g = graph([node("a", state: :running)])

      g = Graph.record_finished_attempt(g, "a", {:ok, %{}}, "thread-9")

      assert [%Attempt{n: 1, state: :succeeded, thread_id: "thread-9"}] = g.nodes["a"].attempts
    end

    test "record_finished_attempt on an unknown node is a no-op" do
      g = graph([node("a", state: :pending)])
      assert Graph.record_finished_attempt(g, "nope", {:ok, %{}}, nil) == g
    end

    test "record_attempt_thread_id stamps only a running node's open attempt" do
      g = graph([node("a", state: :pending)])
      g = Graph.mark_running(g, g.nodes["a"], 1)

      g = Graph.record_attempt_thread_id(g, "a", "mid-flight")
      assert [%Attempt{thread_id: "mid-flight"}] = g.nodes["a"].attempts

      # A node with no open attempt, a non-running node, and an unknown id
      # are all no-ops: the terminal path already recorded the handle.
      fresh = graph([node("b", state: :pending)])
      assert Graph.record_attempt_thread_id(fresh, "b", "x") == fresh
      assert Graph.record_attempt_thread_id(fresh, "nope", "x") == fresh
    end

    test "mark_attempt_stranded closes the current attempt as stranded" do
      g = graph([node("a", state: :pending)])
      g = Graph.mark_running(g, g.nodes["a"], 1)

      g = Graph.mark_attempt_stranded(g, g.nodes["a"])

      assert [%Attempt{n: 1, state: :stranded, outcome: :stranded}] = g.nodes["a"].attempts
    end

    test "mark_attempt_stranded synthesizes an attempt when none was recorded" do
      g = graph([node("a", state: :running)])

      g = Graph.mark_attempt_stranded(g, g.nodes["a"])

      assert [%Attempt{n: 1, state: :stranded, outcome: :stranded}] = g.nodes["a"].attempts
    end

    test "transition sets the node state without touching output or attempts" do
      g = graph([node("a", state: :pending)])
      g = Graph.mark_running(g, g.nodes["a"], 1)

      g = Graph.transition(g, "a", :stranded)

      assert g.nodes["a"].state == :stranded
      assert [%Attempt{state: :running}] = g.nodes["a"].attempts
      assert g.nodes["a"].output == nil

      # An unknown id is a no-op, matching the other bookkeeping entries.
      assert Graph.transition(g, "nope", :cancelled) == g
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
