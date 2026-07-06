defmodule SymphonyElixirWeb.Components.IRGraphTest do
  use ExUnit.Case, async: true

  alias SymphonyElixirWeb.Components.IRGraph

  # Build a minimal node map matching the shape IR.View.render_node/1 produces.
  # Named mk_node to avoid conflicting with Kernel.node/1.
  defp mk_node(id, opts \\ []) do
    kind = Keyword.get(opts, :kind, "exec")
    state = Keyword.get(opts, :state, "pending")
    deps = Keyword.get(opts, :deps, [])
    label = Keyword.get(opts, :label, id)

    %{
      "id" => id,
      "kind" => kind,
      "state" => state,
      "deps" => deps,
      "label" => label,
      "envelope" => nil,
      "attempts" => [],
      "output" => nil,
      "updated_at" => nil
    }
  end

  defp mk_agent(id, engine, opts \\ []) do
    model = Keyword.get(opts, :model, nil)
    effort = Keyword.get(opts, :effort, nil)
    permissions = Keyword.get(opts, :permissions, nil)
    location = Keyword.get(opts, :location, nil)
    skill = Keyword.get(opts, :skill, id)

    base = mk_node(id, Keyword.merge([kind: "agent", label: skill], opts))

    envelope =
      %{"engine" => engine}
      |> maybe_put("model", model)
      |> maybe_put("effort", effort)
      |> maybe_put("permissions", permissions)
      |> maybe_put("location", location)

    Map.put(base, "envelope", envelope)
  end

  defp maybe_put(map, _key, nil), do: map
  defp maybe_put(map, key, value), do: Map.put(map, key, value)

  describe "layout/1 layer assignment" do
    test "a root node with no deps is in layer 0" do
      %{nodes: nodes} = IRGraph.layout([mk_node("a")])
      [n] = Enum.reject(nodes, &(&1.state_class == "gtrigger"))
      assert n.id == "a"
      # Layer 0 nodes are positioned at pad_x (30).
      assert n.x == 30
    end

    test "a -> b places a in layer 0 and b in layer 1" do
      layout = IRGraph.layout([mk_node("a"), mk_node("b", deps: ["a"])])
      by_id = Map.new(layout.nodes, &{&1.id, &1})

      assert by_id["a"].x < by_id["b"].x
    end

    test "a -> b, a -> c, b -> d places roots in layer 0 and d in the last layer" do
      nodes = [
        mk_node("a"),
        mk_node("b", deps: ["a"]),
        mk_node("c", deps: ["a"]),
        mk_node("d", deps: ["b"])
      ]

      layout = IRGraph.layout(nodes)
      by_id = Map.new(layout.nodes, &{&1.id, &1})

      # a is a root (layer 0)
      a_x = by_id["a"].x
      # b and c depend on a (layer 1)
      b_x = by_id["b"].x
      c_x = by_id["c"].x
      # d depends on b (layer 2)
      d_x = by_id["d"].x

      assert a_x < b_x
      assert b_x == c_x
      assert d_x > b_x
    end

    test "emits one edge per dep" do
      nodes = [
        mk_node("a"),
        mk_node("b", deps: ["a"]),
        mk_node("c", deps: ["a"]),
        mk_node("d", deps: ["b"])
      ]

      layout = IRGraph.layout(nodes)
      # a->b, a->c, b->d = 3 edges
      assert length(layout.edges) == 3
    end

    test "parallel independent roots all land in layer 0" do
      nodes = [mk_node("x"), mk_node("y"), mk_node("z")]
      layout = IRGraph.layout(nodes)
      by_id = Map.new(layout.nodes, &{&1.id, &1})

      # All roots at the same x
      assert by_id["x"].x == by_id["y"].x
      assert by_id["y"].x == by_id["z"].x
    end

    test "an empty list with no trigger returns a minimal viewbox and no nodes or edges" do
      %{viewbox: vb, nodes: ns, edges: es} = IRGraph.layout([])
      assert vb =~ "0 0"
      assert ns == []
      assert es == []
    end

    test "an empty list with a trigger returns a single trigger node" do
      %{nodes: ns, edges: es} = IRGraph.layout([], "manual")
      assert length(ns) == 1
      assert hd(ns).state_class == "gtrigger"
      assert hd(ns).label == "manual"
      assert es == []
    end

    test "dangling dep edges (dep not in graph) are silently skipped" do
      nodes = [mk_node("b", deps: ["ghost"])]
      layout = IRGraph.layout(nodes)
      # b has no known deps so it is a root
      assert length(layout.nodes) == 1
      assert layout.edges == []
    end
  end

  describe "layout/1 state classes" do
    test "succeeded state produces succeeded class" do
      %{nodes: nodes} = IRGraph.layout([mk_node("a", state: "succeeded")])
      [n] = nodes
      assert n.state_class == "succeeded"
    end

    test "running state produces running class" do
      %{nodes: nodes} = IRGraph.layout([mk_node("a", state: "running")])
      [n] = nodes
      assert n.state_class == "running"
    end

    test "gate kind gets gate prefix in state class" do
      %{nodes: nodes} = IRGraph.layout([mk_node("g", kind: "gate", state: "pending")])
      [n] = nodes
      assert n.state_class == "gate pending"
    end

    test "unknown state falls back to pending class" do
      %{nodes: nodes} = IRGraph.layout([mk_node("a", state: "upstream_failed")])
      [n] = nodes
      assert n.state_class == "pending"
    end
  end

  describe "layout/1 labels" do
    test "node label comes from the label field" do
      %{nodes: nodes} = IRGraph.layout([mk_node("agent-0", label: "my_skill")])
      [n] = nodes
      assert n.label == "my_skill"
    end

    test "node id is exposed separately from label" do
      %{nodes: nodes} = IRGraph.layout([mk_node("agent-0", label: "my_skill")])
      [n] = nodes
      assert n.id == "agent-0"
    end

    test "node without label field falls back to the id" do
      node = "fallback-id" |> mk_node() |> Map.delete("label")
      %{nodes: nodes} = IRGraph.layout([node])
      [n] = nodes
      assert n.label == "fallback-id"
    end
  end

  describe "layout/1 detail lines for agent nodes" do
    test "agent node with full envelope produces engine/model, effort, permissions, location lines" do
      node =
        mk_agent("agent-0", "codex",
          model: "gpt-5.5",
          effort: "high",
          permissions: "danger_full_access",
          location: "ixvm",
          skill: "my_skill"
        )

      %{nodes: nodes} = IRGraph.layout([node])
      [n] = nodes
      assert "codex gpt-5.5" in n.detail_lines
      assert "high" in n.detail_lines
      assert "danger_full_access" in n.detail_lines
      assert "ixvm" in n.detail_lines
    end

    test "agent node without model shows engine only in first detail line" do
      node = mk_agent("agent-0", "codex", skill: "s")
      %{nodes: nodes} = IRGraph.layout([node])
      [n] = nodes
      assert "codex" in n.detail_lines
    end

    test "exec node detail shows exec kind" do
      %{nodes: nodes} = IRGraph.layout([mk_node("e", kind: "exec", label: "./run.sh")])
      [n] = nodes
      assert n.detail_lines == ["exec"]
    end

    test "gate node detail shows gate" do
      %{nodes: nodes} = IRGraph.layout([mk_node("g", kind: "gate")])
      [n] = nodes
      assert n.detail_lines == ["gate"]
    end

    test "agent location annotates the fallback when effective placement differs" do
      node = mk_agent("agent-0", "codex", location: "ixvm", skill: "s")
      placement = %{"declared" => "ixvm", "effective" => "host"}
      %{nodes: nodes} = IRGraph.layout([node], "manual", placement)
      n = Enum.find(nodes, &(&1.id == "agent-0"))
      assert "ixvm (fallback host)" in n.detail_lines
      refute "ixvm" in n.detail_lines
    end

    test "agent location shows no fallback when effective matches the declared type" do
      node = mk_agent("agent-0", "codex", location: "host:hil-compute-2", skill: "s")
      placement = %{"declared" => "host:hil-compute-2", "effective" => "host"}
      %{nodes: nodes} = IRGraph.layout([node], "manual", placement)
      n = Enum.find(nodes, &(&1.id == "agent-0"))
      assert "host:hil-compute-2" in n.detail_lines
    end
  end

  describe "layout/1 trigger node" do
    test "trigger produces a gtrigger node in the output" do
      nodes = [mk_node("a")]
      layout = IRGraph.layout(nodes, "cron 30 * * * *")
      trigger_nodes = Enum.filter(layout.nodes, &(&1.state_class == "gtrigger"))
      assert length(trigger_nodes) == 1
      assert hd(trigger_nodes).label == "cron 30 * * * *"
    end

    test "trigger node is positioned to the left of root real nodes" do
      nodes = [mk_node("a")]
      layout = IRGraph.layout(nodes, "manual")
      by_id = Map.new(layout.nodes, &{&1.id, &1})
      assert by_id["__trigger__"].x < by_id["a"].x
    end

    test "trigger produces edges to each root node" do
      nodes = [mk_node("a"), mk_node("b")]
      layout = IRGraph.layout(nodes, "manual")
      # 2 roots => 2 trigger edges (a->b has no dep so both are roots)
      assert length(layout.edges) == 2
    end

    test "trigger does not add extra edges to non-root nodes" do
      # b depends on a, so only a is a root; trigger has one edge to a, and
      # one dep edge a->b gives 2 total
      nodes = [mk_node("a"), mk_node("b", deps: ["a"])]
      layout = IRGraph.layout(nodes, "cron 0 * * * *")
      assert length(layout.edges) == 2
    end

    test "layout without trigger has no gtrigger nodes" do
      nodes = [mk_node("a"), mk_node("b", deps: ["a"])]
      layout = IRGraph.layout(nodes)
      trigger_nodes = Enum.filter(layout.nodes, &(&1.state_class == "gtrigger"))
      assert trigger_nodes == []
    end
  end

  describe "layout box sizing" do
    test "node width grows to fit a long label so it does not spill" do
      long = "cron 0 0,5,10,15,20 * * *"
      layout = IRGraph.layout([mk_node("a")], long)
      # The box must be wide enough for the long trigger label plus padding so
      # the text stays inside the rect (regression for the graph-spillage bug).
      assert layout.node_w >= String.length(long) * 7 + 20
    end

    test "node height grows to fit the full envelope block" do
      node =
        mk_agent("agent-0", "codex",
          model: "gpt-5.5",
          effort: "high",
          permissions: "danger_full_access",
          location: "ixvm",
          skill: "idiomatic"
        )

      layout = IRGraph.layout([node])
      # label + id + four envelope detail lines must fit inside the box.
      assert layout.node_h >= 44 + 3 * 13 + 6
    end
  end

  describe "layout/1 single-node no-stretch" do
    test "single node layout natural_width is bounded (not stretched to fill)" do
      layout = IRGraph.layout([mk_node("a")])
      # The natural width of a single-node layout should be much less than a
      # typical screen width. Two pad_x margins plus one node width is the
      # expected value. It must be less than 400 (no card-fill stretch).
      assert layout.natural_width < 400
    end

    test "single node with trigger natural_width is bounded" do
      layout = IRGraph.layout([mk_node("a")], "manual")
      assert layout.natural_width < 600
    end

    test "viewBox width equals natural_width for single-node layout" do
      layout = IRGraph.layout([mk_node("a")])
      "0 0 " <> rest = layout.viewbox
      [w_str | _] = String.split(rest, " ")
      {vb_width, _} = Integer.parse(w_str)
      assert vb_width == layout.natural_width
    end
  end

  describe "layout/1 multi-node trigger -> route -> skill" do
    test "three-layer trigger-route-skill graph lays out left-to-right" do
      # route depends on nothing (root), skill depends on route
      nodes = [
        mk_agent("route-0", "codex", skill: "route", deps: []),
        mk_agent("skill-0", "codex", skill: "idiomatic", deps: ["route-0"])
      ]

      layout = IRGraph.layout(nodes, "cron 30 * * * *")
      by_id = Map.new(layout.nodes, &{&1.id, &1})

      # trigger -> route-0 -> skill-0 must be strictly left-to-right
      assert by_id["__trigger__"].x < by_id["route-0"].x
      assert by_id["route-0"].x < by_id["skill-0"].x
    end

    test "three-layer graph has trigger edge plus dep edge (2 total)" do
      nodes = [
        mk_agent("route-0", "codex", skill: "route"),
        mk_agent("skill-0", "codex", skill: "idiomatic", deps: ["route-0"])
      ]

      layout = IRGraph.layout(nodes, "cron 30 * * * *")
      # trigger->route-0 and route-0->skill-0
      assert length(layout.edges) == 2
    end
  end

  describe "layout/1 edge path format" do
    test "each edge has a non-empty d attribute starting with M" do
      layout = IRGraph.layout([mk_node("a"), mk_node("b", deps: ["a"])])
      assert [%{d: d}] = layout.edges
      assert String.starts_with?(d, "M")
    end
  end

  describe "single cron-triggered agent with full envelope" do
    test "layout contains trigger label, skill name, engine+model, effort, permissions, location" do
      node =
        mk_agent("agent-0", "codex",
          model: "gpt-5.5",
          effort: "high",
          permissions: "danger_full_access",
          location: "ixvm",
          skill: "idiomatic"
        )

      layout = IRGraph.layout([node], "cron 30 * * * *")
      by_id = Map.new(layout.nodes, &{&1.id, &1})

      # Trigger node has the cron label
      assert by_id["__trigger__"].label == "cron 30 * * * *"

      # Agent node primary label is the skill name
      agent = by_id["agent-0"]
      assert agent.label == "idiomatic"

      # Agent node secondary id is distinct from label
      assert agent.id == "agent-0"

      # Envelope detail lines contain engine+model, effort, permissions, location
      assert "codex gpt-5.5" in agent.detail_lines
      assert "high" in agent.detail_lines
      assert "danger_full_access" in agent.detail_lines
      assert "ixvm" in agent.detail_lines
    end
  end
end
