defmodule SymphonyElixir.IR.MaterializerTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.DSL.Parser
  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Graph
  alias SymphonyElixir.IR.Materializer
  alias SymphonyElixir.IR.RunGraph

  defp parse!(source) do
    {:ok, ast} = Parser.parse(source)
    ast
  end

  describe "materialize/3" do
    test "builds a running RunGraph with the static nodes and a lowered envelope" do
      ast =
        parse!("""
        workflow "w" {
          run <- agent { engine: codex, model: "gpt-5.3-codex", permissions: workspace_write, prompt: inline "go" }
        }
        """)

      assert {:ok, graph} = Materializer.materialize("run_1", "hash", ast)
      assert graph.status == :running
      assert [node] = Map.values(graph.nodes)
      assert node.kind == :agent
      # The raw spec map is lowered to a typed Engine.Envelope at this boundary.
      assert %Envelope{engine: :codex, model: "gpt-5.3-codex", permissions: :workspace_write} = node.envelope
    end

    test "an invalid envelope fails the whole materialization with the node id" do
      # A claude-looking model under engine: codex is a load error.
      ast =
        parse!("""
        workflow "w" {
          run <- agent { engine: codex, model: "claude-opus-4", prompt: inline "go" }
        }
        """)

      assert {:error, {:invalid_envelope, "agent-0", {:engine_model_mismatch, :codex, "claude-opus-4"}}} =
               Materializer.materialize("run_1", "hash", ast)
    end
  end

  describe "expand_dynamic/1" do
    test "a when-gate emits its body once the gating output is known" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "first" }
          when ${a.ok} {
            b <- agent { engine: codex, model: "m", prompt: inline "second" }
          }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)

      # Before a's output is known, the gate is a placeholder and the body
      # agent node is absent (only agent-0 plus the gate exist).
      assert Enum.any?(Map.values(graph.nodes), &(&1.kind == :gate))
      agents_before = for {id, %{kind: :agent}} <- graph.nodes, do: id
      assert agents_before == ["agent-0"]

      # Succeed a with a truthy `ok`, then re-expand.
      graph = Graph.apply_output(graph, "agent-0", {:ok, %{"ok" => true}})
      assert {:ok, expanded, new_ids} = Materializer.expand_dynamic(graph)

      # Exactly one new agent node (the gate body) appears.
      assert [body_id] = new_ids
      assert expanded.nodes[body_id].kind == :agent
      # The resolved gate placeholder is retired so it cannot deadlock the run.
      gate = Enum.find(Map.values(expanded.nodes), &(&1.kind == :gate))
      assert gate.state == :skipped
    end

    test "a falsey when-gate emits no body and retires the placeholder" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "first" }
          when ${a.ok} {
            b <- agent { engine: codex, model: "m", prompt: inline "second" }
          }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)
      graph = Graph.apply_output(graph, "agent-0", {:ok, %{"ok" => false}})

      assert {:ok, expanded, new_ids} = Materializer.expand_dynamic(graph)
      assert new_ids == []
      refute Map.has_key?(expanded.nodes, "agent-1")
      assert Enum.find(Map.values(expanded.nodes), &(&1.kind == :gate)).state == :skipped
    end

    test "re-expansion is idempotent: a second pass adds nothing new" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "first" }
          when ${a.ok} {
            b <- agent { engine: codex, model: "m", prompt: inline "second" }
          }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)
      graph = Graph.apply_output(graph, "agent-0", {:ok, %{"ok" => true}})

      {:ok, once, _} = Materializer.expand_dynamic(graph)
      {:ok, twice, second_ids} = Materializer.expand_dynamic(once)

      assert second_ids == []
      assert once.nodes |> Map.keys() |> Enum.sort() == twice.nodes |> Map.keys() |> Enum.sort()
    end

    test "a map fan-out emits one child per element and retires the placeholder" do
      ast =
        parse!("""
        workflow "w" {
          seed <- agent { engine: codex, model: "m", prompt: inline "list" }
          map ${seed.repos} as repo {
            child <- exec "./audit.sh" { target: ${repo} }
          }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)

      # Before the list is known the fan-out is a single placeholder and no
      # body child exists.
      assert Enum.any?(Map.values(graph.nodes), &(&1.kind == :map_fanout))
      refute Enum.any?(Map.values(graph.nodes), &(&1.kind == :exec))

      graph = Graph.apply_output(graph, "agent-0", {:ok, %{"repos" => ["alpha", "beta", "gamma"]}})
      assert {:ok, expanded, new_ids} = Materializer.expand_dynamic(graph)

      # One exec child per element, each carrying its element literally, with
      # distinct content-derived ids.
      children = for {_id, %{kind: :exec} = n} <- expanded.nodes, do: n
      assert length(children) == 3
      assert length(new_ids) == 3
      targets = children |> Enum.map(& &1.inputs["target"]) |> Enum.sort()
      assert targets == [{:literal, "alpha"}, {:literal, "beta"}, {:literal, "gamma"}]

      # The resolved fan-out placeholder is retired so it cannot deadlock the run.
      assert Enum.find(Map.values(expanded.nodes), &(&1.kind == :map_fanout)).state == :skipped
    end

    test "re-expanding a fanned-out map merges idempotently, adding nothing new" do
      ast =
        parse!("""
        workflow "w" {
          seed <- agent { engine: codex, model: "m", prompt: inline "list" }
          map ${seed.repos} as repo {
            child <- exec "./audit.sh" { target: ${repo} }
          }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)
      graph = Graph.apply_output(graph, "agent-0", {:ok, %{"repos" => ["alpha", "beta"]}})

      {:ok, once, first_ids} = Materializer.expand_dynamic(graph)
      assert length(first_ids) == 2

      # A second pass re-emits the same children (the interpreter re-derives
      # them deterministically), but the merge-by-id adds nothing because the
      # ids already exist. This confirms the Phase 7 agent's belief that
      # re-emitting children on each pass merges idempotently.
      {:ok, twice, second_ids} = Materializer.expand_dynamic(once)
      assert second_ids == []
      assert once.nodes |> Map.keys() |> Enum.sort() == twice.nodes |> Map.keys() |> Enum.sort()
    end

    test "a map over an empty list emits no children and retires the placeholder" do
      ast =
        parse!("""
        workflow "w" {
          seed <- agent { engine: codex, model: "m", prompt: inline "list" }
          map ${seed.repos} as repo {
            child <- exec "./audit.sh" { target: ${repo} }
          }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)
      graph = Graph.apply_output(graph, "agent-0", {:ok, %{"repos" => []}})

      assert {:ok, expanded, new_ids} = Materializer.expand_dynamic(graph)
      assert new_ids == []
      refute Enum.any?(Map.values(expanded.nodes), &(&1.kind == :exec))
      assert Enum.find(Map.values(expanded.nodes), &(&1.kind == :map_fanout)).state == :skipped
    end

    test "a graph without a workflow AST is returned unchanged" do
      graph = RunGraph.new("run_1", "hash", nil)
      assert {:ok, ^graph, []} = Materializer.expand_dynamic(graph)
    end

    test "a deferred inline prompt waits on its input and folds to text once the output arrives" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "first" }
          b <- agent { engine: codex, model: "m", prompt: inline "use ${a.result} now" }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)

      # b interpolates a's output. The interpreter cannot fold the mixed
      # literal/node concat into one input ref, so the edge arrives via the
      # pending set; the materializer must still make b depend on a so it
      # does not run with an unresolved prompt.
      b = graph.nodes["agent-1"]
      assert b.prompt_ref == {:inline, nil}
      assert "agent-0" in b.deps
      refute Enum.any?(Graph.ready_nodes(graph), &(&1.id == "agent-1"))

      graph = Graph.apply_output(graph, "agent-0", {:ok, %{"result" => "X"}})
      assert {:ok, expanded, _ids} = Materializer.expand_dynamic(graph)

      b = expanded.nodes["agent-1"]
      assert b.prompt_ref == {:inline, "use X now"}
      assert b.state == :pending
      # The edge is kept for provenance even though the prompt now folds to a
      # literal; it points at the succeeded agent-0 so b is schedulable.
      assert "agent-0" in b.deps
      assert Enum.any?(Graph.ready_nodes(expanded), &(&1.id == "agent-1"))
    end

    test "a deferred skill binding folds to the resolved value on re-expansion" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "first" }
          b <- agent { engine: codex, model: "m", prompt: skill "next" { ctx: ${a.area} } }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)

      # A skill binding that reads a node output already carries the edge as
      # an input ref, so b depends on a from the first pass.
      b = graph.nodes["agent-1"]
      assert "agent-0" in b.deps
      assert b.inputs["ctx"] == {:node, "agent-0", ["area"]}
      assert {:skill, "next", %{"ctx" => unresolved}} = b.prompt_ref
      refute unresolved == "DB"

      graph = Graph.apply_output(graph, "agent-0", {:ok, %{"area" => "DB"}})
      assert {:ok, expanded, _ids} = Materializer.expand_dynamic(graph)

      b = expanded.nodes["agent-1"]
      assert b.prompt_ref == {:skill, "next", %{"ctx" => "DB"}}
      assert b.inputs["ctx"] == {:literal, "DB"}
    end

    test "a node already running is not clobbered by re-expansion" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "first" }
          b <- agent { engine: codex, model: "m", prompt: inline "use ${a.result} now" }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)
      # Force agent-0 into a live state, then re-expand: a running/terminal
      # node keeps its state and is never replaced by the fresh expansion.
      graph = put_in(graph.nodes["agent-0"].state, :running)

      assert {:ok, expanded, new_ids} = Materializer.expand_dynamic(graph)
      assert new_ids == []
      assert expanded.nodes["agent-0"].state == :running
    end
  end

  describe "known_outputs/1" do
    test "exposes only succeeded node outputs" do
      ast =
        parse!("""
        workflow "w" {
          a <- agent { engine: codex, model: "m", prompt: inline "first" }
        }
        """)

      {:ok, graph} = Materializer.materialize("run_1", "hash", ast)
      assert Materializer.known_outputs(graph) == %{}

      graph = Graph.apply_output(graph, "agent-0", {:ok, %{"area" => 42}})
      assert Materializer.known_outputs(graph) == %{"agent-0" => %{"area" => 42}}
    end
  end
end
