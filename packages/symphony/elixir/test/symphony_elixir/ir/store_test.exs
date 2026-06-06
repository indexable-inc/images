defmodule SymphonyElixir.IR.StoreTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.{Attempt, Node, RunGraph, Store}

  setup do
    dir = Path.join(System.tmp_dir!(), "ir_store_test_#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    on_exit(fn -> File.rm_rf(dir) end)
    {:ok, dir: dir}
  end

  defp sample_graph do
    {:ok, env} =
      Envelope.from_map(%{"engine" => "codex", "model" => "gpt-5.3-codex", "effort" => "medium"})

    agent =
      Node.new(
        id: "agent-1",
        ast_origin: {:agent, "write"},
        kind: :agent,
        envelope: env,
        prompt_ref: {:skill, "writer", %{"topic" => "hello"}},
        inputs: %{"seed" => {:literal, 42}},
        state: :running
      )

    attempt = Attempt.start(1, :codex, "thread-abc") |> Attempt.finish(:succeeded, :ok, %{usd: 0.12, tokens_in: 100})
    agent = %{agent | attempts: [attempt]}

    exec =
      Node.new(
        id: "exec-1",
        ast_origin: {:exec, "build"},
        kind: :exec,
        inputs: %{"from" => {:node, "agent-1", [:output]}},
        state: :pending
      )

    RunGraph.new("run-store-1", "deadbeef", {:ast, [:root]})
    |> RunGraph.put_nodes([agent, exec])
    |> RunGraph.append_expansion({:gate, "g1"}, {:observed, true}, ["exec-1"])
  end

  test "round-trips a RunGraph with attempts and an expansion log", %{dir: dir} do
    graph = sample_graph()

    assert :ok = Store.persist(graph, dir: dir)
    assert {:ok, loaded} = Store.load(graph.run_id, dir: dir)

    assert loaded.run_id == graph.run_id
    assert loaded.source_hash == graph.source_hash
    assert loaded.ast == {:ast, [:root]}
    assert loaded.status == graph.status

    agent = loaded.nodes["agent-1"]
    assert agent.kind == :agent
    assert agent.state == :running
    assert agent.envelope.engine == :codex
    assert agent.envelope.model == "gpt-5.3-codex"
    assert agent.prompt_ref == {:skill, "writer", %{"topic" => "hello"}}
    assert agent.inputs == %{"seed" => {:literal, 42}}

    [att] = agent.attempts
    assert att.thread_id == "thread-abc"
    assert att.state == :succeeded
    assert att.outcome == :ok
    assert att.cost == %{usd: 0.12, tokens_in: 100}

    exec = loaded.nodes["exec-1"]
    assert exec.inputs == %{"from" => {:node, "agent-1", [:output]}}
    assert exec.deps == ["agent-1"]

    [event] = loaded.expansion_log
    assert event.origin == {:gate, "g1"}
    assert event.observed == {:observed, true}
    assert event.emitted == ["exec-1"]
  end

  test "load_all returns every decodable graph and quarantines a corrupt file", %{dir: dir} do
    graph = sample_graph()
    assert :ok = Store.persist(graph, dir: dir)

    bad_path = Path.join(dir, "broken.json")
    File.write!(bad_path, "{ not json")

    loaded = Store.load_all(dir: dir)
    assert Enum.map(loaded, & &1.run_id) == ["run-store-1"]

    refute File.exists?(bad_path)
    assert File.exists?(bad_path <> ".bad")
  end

  test "append_expansion persists the new event", %{dir: dir} do
    graph = sample_graph()
    assert :ok = Store.persist(graph, dir: dir)

    assert {:ok, next} = Store.append_expansion(graph, {{:gate, "g2"}, {:observed, 7}, ["exec-1"]}, dir: dir)
    assert length(next.expansion_log) == 2

    assert {:ok, reloaded} = Store.load(graph.run_id, dir: dir)
    assert length(reloaded.expansion_log) == 2
  end

  test "load returns :not_found for an unknown run", %{dir: dir} do
    assert {:error, :not_found} = Store.load("nope", dir: dir)
  end

  test "round-trips a graph with a placement map (ixvm declared, host effective)", %{dir: dir} do
    graph =
      RunGraph.new("run-placement", "deadbeef", nil)
      |> Map.put(:placement, %{declared: :ixvm, effective: :host})

    assert :ok = Store.persist(graph, dir: dir)
    assert {:ok, loaded} = Store.load("run-placement", dir: dir)

    assert loaded.placement == %{declared: :ixvm, effective: :host}
  end

  test "round-trips a graph with a remote effective placement (ixvm -> remote fallback)", %{dir: dir} do
    graph =
      RunGraph.new("run-placement-remote", "deadbeef", nil)
      |> Map.put(:placement, %{declared: :ixvm, effective: :remote})

    assert :ok = Store.persist(graph, dir: dir)
    assert {:ok, loaded} = Store.load("run-placement-remote", dir: dir)

    assert loaded.placement == %{declared: :ixvm, effective: :remote}
  end

  test "round-trips a graph with a host-named declared placement", %{dir: dir} do
    graph =
      RunGraph.new("run-placement-host-named", "deadbeef", nil)
      |> Map.put(:placement, %{declared: {:host, "box1"}, effective: :host})

    assert :ok = Store.persist(graph, dir: dir)
    assert {:ok, loaded} = Store.load("run-placement-host-named", dir: dir)

    assert loaded.placement == %{declared: {:host, "box1"}, effective: :host}
  end

  test "round-trips a graph with nil placement (no placement acquired)", %{dir: dir} do
    graph = RunGraph.new("run-no-placement", "deadbeef", nil)

    assert :ok = Store.persist(graph, dir: dir)
    assert {:ok, loaded} = Store.load("run-no-placement", dir: dir)

    assert loaded.placement == nil
  end
end
