defmodule SymphonyElixir.IR.ViewTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Attempt
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.IR.View

  defp agent_node do
    {:ok, env} = Envelope.validate(%Envelope{engine: :codex, model: "gpt-5.3-codex", effort: :high, location: :local})

    attempt =
      1
      |> Attempt.start(:codex, "thread-1")
      |> Attempt.finish(:succeeded, :ok)
      |> Map.put(:cost, %{usd: 0.42, tokens_in: 100, tokens_out: 20})

    %{
      Node.new(id: "a", ast_origin: {:agent, "skill"}, kind: :agent, envelope: env, inputs: %{})
      | state: :succeeded,
        output: %{"area" => 7},
        attempts: [attempt]
    }
  end

  defp graph do
    "run_v"
    |> RunGraph.new("hash", nil)
    |> RunGraph.put_nodes([agent_node()])
    |> Map.put(:status, :succeeded)
    |> RunGraph.append_audit(:retry_node, "a", "alice", %{})
  end

  test "summary/1 reports status, counts, and total cost" do
    s = View.summary(graph())
    assert s["run_id"] == "run_v"
    assert s["status"] == "succeeded"
    assert s["node_count"] == 1
    assert s["states"] == %{"succeeded" => 1}
    assert s["cost_usd"] == 0.42
  end

  test "summary cost is nil when no attempt reported a cost" do
    g = "r" |> RunGraph.new("h", nil) |> RunGraph.put_nodes([Node.new(id: "x", ast_origin: {:exec, "x"}, kind: :exec, inputs: %{})])
    assert View.summary(g)["cost_usd"] == nil
  end

  test "detail/1 renders nodes, attempts, envelope, and audit log as JSON-able facts" do
    d = View.detail(graph())

    assert [node] = d["nodes"]
    assert node["id"] == "a"
    assert node["kind"] == "agent"
    assert node["state"] == "succeeded"
    assert node["envelope"]["engine"] == "codex"
    assert node["envelope"]["effort"] == "high"
    assert node["envelope"]["location"] == "local"
    assert node["output"] == %{"area" => 7}

    assert [attempt] = node["attempts"]
    assert attempt["n"] == 1
    assert attempt["outcome"] == "ok"
    assert attempt["cost"]["usd"] == 0.42

    assert [audit] = d["audit_log"]
    assert audit["action"] == "retry_node"
    assert audit["target"] == "a"
    assert audit["actor"] == "alice"
  end

  describe "render_node/1 label field" do
    test "agent node with skill prompt_ref uses skill name as label" do
      {:ok, env} = Envelope.validate(%Envelope{engine: :codex, model: "gpt-5", effort: :high, location: :local})

      node =
        Node.new(
          id: "skill-node",
          ast_origin: {:agent, "my_skill"},
          kind: :agent,
          envelope: env,
          prompt_ref: {:skill, "my_skill", %{}},
          inputs: %{}
        )

      assert View.render_node(node)["label"] == "my_skill"
    end

    test "agent node with inline prompt_ref uses 'inline' as label" do
      {:ok, env} = Envelope.validate(%Envelope{engine: :codex, model: "gpt-5", effort: :high, location: :local})

      node =
        Node.new(
          id: "inline-node",
          ast_origin: {:agent, "inline"},
          kind: :agent,
          envelope: env,
          prompt_ref: {:inline, "do something"},
          inputs: %{}
        )

      assert View.render_node(node)["label"] == "inline"
    end

    test "exec node with literal script input uses script path as label" do
      node =
        Node.new(
          id: "exec-node",
          ast_origin: {:exec, "run"},
          kind: :exec,
          inputs: %{"script" => {:literal, "./scripts/deploy.sh"}}
        )

      assert View.render_node(node)["label"] == "./scripts/deploy.sh"
    end

    test "exec node without resolved script input uses 'exec' as label" do
      node =
        Node.new(
          id: "exec-node",
          ast_origin: {:exec, "run"},
          kind: :exec,
          inputs: %{}
        )

      assert View.render_node(node)["label"] == "exec"
    end

    test "gate node uses 'gate' as label" do
      node =
        Node.new(
          id: "gate-node",
          ast_origin: {:gate, "check"},
          kind: :gate,
          inputs: %{}
        )

      assert View.render_node(node)["label"] == "gate"
    end

    test "subrun node uses 'subrun' as label" do
      node =
        Node.new(
          id: "sub-node",
          ast_origin: {:subrun, "child"},
          kind: :subrun,
          inputs: %{}
        )

      assert View.render_node(node)["label"] == "subrun"
    end
  end

  test "the rendered detail encodes to JSON without a custom encoder" do
    assert {:ok, _json} = graph() |> View.detail() |> Jason.encode()
  end

  test "render_node stringifies a non-default location" do
    {:ok, env} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: {:room, "http://h:1"}})
    node = Node.new(id: "n", ast_origin: {:agent, "s"}, kind: :agent, envelope: env, inputs: %{})
    assert View.render_node(node)["envelope"]["location"] == "room:http://h:1"
  end

  describe "summary/1 trigger and placement fields" do
    test "summary includes trigger as a string label for a manual trigger" do
      g = Map.put(graph(), :trigger, %{kind: :manual})
      s = View.summary(g)
      assert s["trigger"] == "manual"
    end

    test "summary includes trigger label for a cron trigger" do
      g = Map.put(graph(), :trigger, %{kind: :cron, schedule: "0 * * * *"})
      s = View.summary(g)
      assert s["trigger"] == "cron 0 * * * *"
    end

    test "summary defaults trigger to 'manual' when trigger is nil" do
      g = RunGraph.new("r-nil-trigger", "h", nil)
      assert View.summary(g)["trigger"] == "manual"
    end

    test "summary includes placement with declared and effective as strings" do
      g = Map.put(graph(), :placement, %{declared: :ixvm, effective: :host})
      s = View.summary(g)
      assert s["placement"] == %{"declared" => "ixvm", "effective" => "host"}
    end

    test "summary includes placement for an ixvm -> host fallback" do
      g = Map.put(graph(), :placement, %{declared: :ixvm, effective: :host})
      s = View.summary(g)
      # A consumer can detect a fallback by comparing declared != effective.
      assert s["placement"]["declared"] == "ixvm"
      assert s["placement"]["effective"] == "host"
    end

    test "summary placement is nil when no placement was acquired" do
      g = RunGraph.new("r-no-placement", "h", nil)
      assert View.summary(g)["placement"] == nil
    end

    test "trigger_label/1 is a public shared formatter" do
      assert View.trigger_label(%{kind: :manual}) == "manual"
      assert View.trigger_label(%{kind: :cron, schedule: "*/5 * * * *"}) == "cron */5 * * * *"
      assert View.trigger_label(%{kind: :linear, label: "bug"}) == "linear: bug"
      assert View.trigger_label(%{kind: :github_pr_label, label: "review"}) == "github: review"
      assert View.trigger_label(nil) == "manual"
    end
  end
end
