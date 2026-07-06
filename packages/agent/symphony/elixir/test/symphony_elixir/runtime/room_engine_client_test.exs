defmodule SymphonyElixir.Runtime.RoomEngineClientTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.Runtime.RoomEngineClient

  defp agent_node(prompt_ref, location \\ :local) do
    {:ok, env} = Envelope.validate(%Envelope{engine: :codex, model: "gpt-5.3-codex", location: location})

    Node.new(
      id: "n0",
      ast_origin: {:agent, "skill"},
      kind: :agent,
      envelope: env,
      prompt_ref: prompt_ref,
      inputs: %{}
    )
  end

  defp ok_plug(thread_id) do
    fn conn ->
      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.send_resp(
        200,
        Jason.encode!(%{"threadId" => thread_id, "outcome" => %{"kind" => "ok"}, "eventCount" => 3})
      )
    end
  end

  test "runs an inline-prompt agent node and returns {:ok, output, thread_id}" do
    node = agent_node({:inline, "write FOO and stop"})

    run_opts = %{
      run_id: "run_1",
      attempt: 1,
      cwd: "/workspace/run_1",
      room_server_url: "http://room.test",
      req_options: [plug: ok_plug("thread_xyz")]
    }

    assert {:ok, %{thread_id: "thread_xyz", event_count: 3}, "thread_xyz"} =
             RoomEngineClient.run_node(node, run_opts)
  end

  test "forwards the node id and run id to the room-server payload" do
    test_pid = self()

    plug = fn conn ->
      {:ok, raw, conn} = Plug.Conn.read_body(conn)
      send(test_pid, {:payload, Jason.decode!(raw)})

      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.send_resp(200, Jason.encode!(%{"threadId" => "t", "outcome" => %{"kind" => "ok"}, "eventCount" => 0}))
    end

    node = agent_node({:inline, "do work"})
    run_opts = %{run_id: "run_42", attempt: 1, cwd: "/w", room_server_url: "http://room.test", req_options: [plug: plug]}

    assert {:ok, _, _} = RoomEngineClient.run_node(node, run_opts)
    assert_received {:payload, payload}
    assert payload["runId"] == "run_42"
    assert payload["nodeId"] == "n0"
    assert payload["prompt"] == "do work"
    assert payload["cwd"] == "/w"
    assert payload["engine"] == "codex"
  end

  test "an error outcome carries the thread id through for a later reattach probe" do
    plug = fn conn ->
      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.send_resp(
        200,
        Jason.encode!(%{"threadId" => "thread_e", "outcome" => %{"kind" => "error", "message" => "boom"}, "eventCount" => 1})
      )
    end

    node = agent_node({:inline, "do work"})
    run_opts = %{run_id: "r", attempt: 1, cwd: "/w", room_server_url: "http://room.test", req_options: [plug: plug]}

    assert {:error, {:turn_error, "boom", "thread_e"}, "thread_e"} = RoomEngineClient.run_node(node, run_opts)
  end

  test "a skill prompt is rendered from the resolved body and bindings" do
    test_pid = self()

    plug = fn conn ->
      {:ok, raw, conn} = Plug.Conn.read_body(conn)
      send(test_pid, {:payload, Jason.decode!(raw)})

      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.send_resp(200, Jason.encode!(%{"threadId" => "t", "outcome" => %{"kind" => "ok"}, "eventCount" => 1}))
    end

    node = agent_node({:skill, "inspect", %{"repo" => "symphony"}})

    run_opts = %{
      run_id: "r",
      attempt: 1,
      cwd: "/w",
      room_server_url: "http://room.test",
      req_options: [plug: plug],
      # Inject the skill body so the test does not need a running Catalog.
      skill_resolver: fn "inspect" -> {:ok, "inspect the ${repo} repo"} end
    }

    assert {:ok, _output, "t"} = RoomEngineClient.run_node(node, run_opts)
    assert_receive {:payload, payload}
    assert payload["prompt"] == "inspect the symphony repo"
  end

  test "appends the run's trigger context as an <input> block on the agent prompt" do
    test_pid = self()

    plug = fn conn ->
      {:ok, raw, conn} = Plug.Conn.read_body(conn)
      send(test_pid, {:payload, Jason.decode!(raw)})

      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.send_resp(200, Jason.encode!(%{"threadId" => "t", "outcome" => %{"kind" => "ok"}, "eventCount" => 0}))
    end

    node = agent_node({:inline, "digest the window"})

    trigger = %{
      kind: :cron,
      scheduled_for: "2026-06-03T07:00:00Z",
      fired_at: "2026-06-03T07:00:07Z",
      input: %{lookback_hours: 5}
    }

    run_opts = %{
      run_id: "r",
      attempt: 1,
      cwd: "/w",
      trigger: trigger,
      room_server_url: "http://room.test",
      req_options: [plug: plug]
    }

    assert {:ok, _, _} = RoomEngineClient.run_node(node, run_opts)
    assert_receive {:payload, payload}

    prompt = payload["prompt"]
    assert String.starts_with?(prompt, "digest the window")
    assert prompt =~ "<input>"
    assert prompt =~ "</input>"
    # The block carries the verbatim trigger envelope the skill reads.
    assert prompt =~ ~s("scheduled_for": "2026-06-03T07:00:00Z")
    assert prompt =~ "\"lookback_hours\": 5"
  end

  test "omits the <input> block for an operator-started run with no trigger" do
    test_pid = self()

    plug = fn conn ->
      {:ok, raw, conn} = Plug.Conn.read_body(conn)
      send(test_pid, {:payload, Jason.decode!(raw)})

      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.send_resp(200, Jason.encode!(%{"threadId" => "t", "outcome" => %{"kind" => "ok"}, "eventCount" => 0}))
    end

    node = agent_node({:inline, "do work"})
    run_opts = %{run_id: "r", attempt: 1, cwd: "/w", trigger: nil, room_server_url: "http://room.test", req_options: [plug: plug]}

    assert {:ok, _, _} = RoomEngineClient.run_node(node, run_opts)
    assert_receive {:payload, payload}
    assert payload["prompt"] == "do work"
    refute payload["prompt"] =~ "<input>"
  end

  test "a skill that names an unbound input fails loudly rather than half-rendering" do
    node = agent_node({:skill, "inspect", %{}})

    run_opts = %{
      run_id: "r",
      attempt: 1,
      cwd: "/w",
      room_server_url: "http://room.test",
      skill_resolver: fn "inspect" -> {:ok, "needs ${missing}"} end
    }

    assert {:error, {:unbound_placeholder, "missing"}, nil} = RoomEngineClient.run_node(node, run_opts)
  end

  test "a missing cwd fails loudly before any request" do
    node = agent_node({:inline, "do work"})
    assert {:error, :missing_cwd, nil} = RoomEngineClient.run_node(node, %{run_id: "r", attempt: 1})
  end

  test "an agent node with no envelope is a wiring error" do
    node = %{agent_node({:inline, "x"}) | envelope: nil}
    assert {:error, {:missing_envelope, "n0"}, nil} = RoomEngineClient.run_node(node, %{run_id: "r", attempt: 1, cwd: "/w"})
  end

  test "a non-agent node never reaches the engine host" do
    exec = Node.new(id: "e0", ast_origin: {:exec, "build"}, kind: :exec, inputs: %{})
    assert {:error, {:not_an_agent_node, :exec, "e0"}, nil} = RoomEngineClient.run_node(exec, %{run_id: "r", cwd: "/w"})
  end

  test "status/1 is conservatively unknown on the synchronous path" do
    assert RoomEngineClient.status("any-thread") == :unknown
    assert RoomEngineClient.status(nil) == :unknown
  end
end
