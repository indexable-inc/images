defmodule SymphonyElixir.Engine.ClientTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Engine.{Client, Envelope}

  describe "request_body/2" do
    test "lowers a codex envelope to the camelCase TurnRequest wire shape" do
      {:ok, env} =
        Envelope.validate(%Envelope{
          engine: :codex,
          model: "gpt-5.3-codex",
          effort: :high,
          permissions: :workspace_write,
          location: :local
        })

      assert {:ok, body} =
               Client.request_body(env, %{
                 prompt: "write FOO to ./hello.txt and stop.",
                 cwd: "/workspace",
                 run_id: "run_1",
                 node_id: "n0"
               })

      assert body == %{
               "engine" => "codex",
               "model" => "gpt-5.3-codex",
               "effort" => "high",
               "permissions" => "workspace_write",
               "cwd" => "/workspace",
               "prompt" => "write FOO to ./hello.txt and stop.",
               "tools" => [],
               "runId" => "run_1",
               "nodeId" => "n0"
             }
    end

    test "omits effort when the envelope leaves it nil" do
      {:ok, env} =
        Envelope.validate(%Envelope{engine: :claude, model: "haiku", permissions: :danger_full_access, location: :local})

      assert {:ok, body} = Client.request_body(env, %{prompt: "hi", cwd: "/w"})
      refute Map.has_key?(body, "effort")
      assert body["engine"] == "claude"
      assert body["permissions"] == "danger_full_access"
    end

    test "drops nil correlation ids rather than sending null" do
      {:ok, env} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: :local})
      assert {:ok, body} = Client.request_body(env, %{prompt: "hi", cwd: "/w"})
      refute Map.has_key?(body, "runId")
      refute Map.has_key?(body, "nodeId")
    end

    test "rejects a turn missing the prompt or cwd" do
      {:ok, env} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: :local})
      assert {:error, :missing_prompt} = Client.request_body(env, %{cwd: "/w"})
      assert {:error, :missing_cwd} = Client.request_body(env, %{prompt: "hi"})
    end
  end

  describe "submit_turn/3 location resolution" do
    test "a host location resolves to the run's per-run room-server from the placement module" do
      test_pid = self()

      # The run's `Runtime.Placement` provisioned a host room-server (a
      # systemd-run unit) and registered its loopback URL under run_id. The
      # client reads it back the same way it resolves :ixvm; no real unit.
      defmodule HostPlacement do
        def base_url("run_host"), do: {:ok, "http://127.0.0.1:41234"}
        def base_url(_), do: :error
      end

      {:ok, host} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: {:host, "box"}})

      plug = fn conn ->
        send(test_pid, {:hit, conn.host, conn.port})
        respond(conn, %{"threadId" => "t", "outcome" => %{"kind" => "ok"}, "eventCount" => 0})
      end

      assert {:ok, _} =
               Client.submit_turn(host, %{prompt: "hi", cwd: "/w"},
                 room_server_url: "http://ignored.default",
                 run_id: "run_host",
                 placement: HostPlacement,
                 req_options: [plug: plug]
               )

      assert_received {:hit, "127.0.0.1", 41_234}
    end

    test "a host location with no acquired placement fails loudly rather than routing to the default" do
      defmodule UnresolvedHostPlacement do
        def base_url(_run_id), do: :error
      end

      {:ok, host} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: {:host, "box"}})

      assert {:error, {:unresolved_location, {:host, "box"}}} =
               Client.submit_turn(host, %{prompt: "hi", cwd: "/w"},
                 room_server_url: "http://ignored.default",
                 run_id: "run_unknown",
                 placement: UnresolvedHostPlacement
               )
    end

    test "a host location without a run_id is unresolved (no context to look up)" do
      {:ok, host} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: {:host, "box"}})

      assert {:error, {:unresolved_location, {:host, "box"}}} =
               Client.submit_turn(host, %{prompt: "hi", cwd: "/w"}, room_server_url: "http://ignored.default")
    end

    test "an ixvm location resolves to the run's per-run room-server from the placement module" do
      test_pid = self()

      # Stub the placement lookup: the run's `Runtime.Placement` would have
      # provisioned this URL before the first agent turn. No real VM is
      # created; the client just reads the resolved per-run base URL.
      defmodule StubPlacement do
        def base_url("run_42"), do: {:ok, "http://run-42-vm.test:8080"}
        def base_url(_), do: :error
      end

      {:ok, ixvm} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: :ixvm})

      plug = fn conn ->
        send(test_pid, {:hit, conn.host, conn.port})
        respond(conn, %{"threadId" => "t", "outcome" => %{"kind" => "ok"}, "eventCount" => 0})
      end

      assert {:ok, _} =
               Client.submit_turn(ixvm, %{prompt: "hi", cwd: "/w"},
                 room_server_url: "http://ignored.default",
                 run_id: "run_42",
                 placement: StubPlacement,
                 req_options: [plug: plug]
               )

      assert_received {:hit, "run-42-vm.test", 8080}
    end

    test "an ixvm location with no acquired placement fails loudly rather than routing to the default" do
      defmodule UnresolvedPlacement do
        def base_url(_run_id), do: :error
      end

      {:ok, ixvm} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: :ixvm})

      assert {:error, {:unresolved_location, :ixvm}} =
               Client.submit_turn(ixvm, %{prompt: "hi", cwd: "/w"},
                 room_server_url: "http://ignored.default",
                 run_id: "run_unknown",
                 placement: UnresolvedPlacement
               )
    end

    test "an ixvm location without a run_id is unresolved (no context to look up)" do
      {:ok, ixvm} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: :ixvm})

      assert {:error, {:unresolved_location, :ixvm}} =
               Client.submit_turn(ixvm, %{prompt: "hi", cwd: "/w"}, room_server_url: "http://ignored.default")
    end

    test "a local location with no configured url is a clear error" do
      {:ok, env} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: :local})
      assert {:error, :missing_room_server_url} = Client.submit_turn(env, %{prompt: "hi", cwd: "/w"})
    end
  end

  describe "submit_turn/3 against a stub room-server" do
    test "maps an ok outcome to {:ok, %{thread_id, event_count}}" do
      plug = stub_plug(%{"threadId" => "thread_abc", "outcome" => %{"kind" => "ok"}, "eventCount" => 4})
      {:ok, env} = Envelope.validate(%Envelope{engine: :codex, model: "gpt-5.3-codex", location: :local})

      assert {:ok, %{thread_id: "thread_abc", event_count: 4}} =
               Client.submit_turn(env, %{prompt: "hi", cwd: "/w"},
                 room_server_url: "http://room.test",
                 req_options: [plug: plug]
               )
    end

    test "lowers the terminal usage totals to the IR.Attempt cost shape" do
      plug =
        stub_plug(%{
          "threadId" => "thread_abc",
          "outcome" => %{"kind" => "ok"},
          "eventCount" => 4,
          "usage" => %{
            "tokensIn" => 1200,
            "tokensOut" => 340,
            "cacheRead" => 800,
            "cacheCreation" => 64,
            "costUsd" => 0.0123
          }
        })

      {:ok, env} = Envelope.validate(%Envelope{engine: :codex, model: "gpt-5.3-codex", location: :local})

      assert {:ok, %{cost: cost}} =
               Client.submit_turn(env, %{prompt: "hi", cwd: "/w"},
                 room_server_url: "http://room.test",
                 req_options: [plug: plug]
               )

      assert cost == %{
               usd: 0.0123,
               tokens_in: 1200,
               tokens_out: 340,
               cache_read: 800,
               cache_creation: 64
             }
    end

    test "a response without usage records an unknown (nil) cost" do
      plug = stub_plug(%{"threadId" => "t", "outcome" => %{"kind" => "ok"}, "eventCount" => 0})
      {:ok, env} = Envelope.validate(%Envelope{engine: :codex, model: "gpt-5.3-codex", location: :local})

      assert {:ok, %{cost: nil}} =
               Client.submit_turn(env, %{prompt: "hi", cwd: "/w"},
                 room_server_url: "http://room.test",
                 req_options: [plug: plug]
               )
    end

    test "maps an error outcome to {:error, {:turn_error, message, thread_id}}" do
      plug =
        stub_plug(%{
          "threadId" => "thread_err",
          "outcome" => %{"kind" => "error", "message" => "model refused"},
          "eventCount" => 1
        })

      {:ok, env} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: {:room, "http://room.test"}})

      assert {:error, {:turn_error, "model refused", "thread_err"}} =
               Client.submit_turn(env, %{prompt: "hi", cwd: "/w"}, req_options: [plug: plug])
    end

    test "an explicit {:room, url} location overrides the default url" do
      test_pid = self()

      plug = fn conn ->
        send(test_pid, {:hit, conn.host, conn.port})
        respond(conn, %{"threadId" => "t", "outcome" => %{"kind" => "ok"}, "eventCount" => 0})
      end

      {:ok, env} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: {:room, "http://chosen.test:9999"}})

      assert {:ok, _} =
               Client.submit_turn(env, %{prompt: "hi", cwd: "/w"},
                 room_server_url: "http://default.test",
                 req_options: [plug: plug]
               )

      assert_received {:hit, "chosen.test", 9999}
    end

    test "a non-2xx status surfaces as an agent_turn_status error" do
      plug = fn conn -> Plug.Conn.send_resp(conn, 503, "engine claude not configured") end
      {:ok, env} = Envelope.validate(%Envelope{engine: :claude, model: "haiku", location: :local})

      assert {:error, {:agent_turn_status, 503, _}} =
               Client.submit_turn(env, %{prompt: "hi", cwd: "/w"},
                 room_server_url: "http://room.test",
                 req_options: [plug: plug]
               )
    end
  end

  defp stub_plug(json), do: fn conn -> respond(conn, json) end

  defp respond(conn, json) do
    conn
    |> Plug.Conn.put_resp_content_type("application/json")
    |> Plug.Conn.send_resp(200, Jason.encode!(json))
  end
end
