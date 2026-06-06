defmodule SymphonyElixir.Engine.ContractFixturesTest do
  @moduledoc """
  The Elixir half of the cross-language contract guard (see
  `docs/engine-contract.md`). It asserts that `Engine.Client.request_body/2`
  reproduces the shared `contracts/fixtures/turn_request.json` shape the Rust
  room-server parses, so a field rename on either side fails a check rather
  than drifting silently at runtime.
  """
  use ExUnit.Case, async: true

  alias SymphonyElixir.Engine.{Client, Envelope}

  # contracts/ sits at the repo root, four levels up from this test file.
  @fixtures Path.expand(Path.join([__DIR__, "..", "..", "..", "..", "contracts", "fixtures"]))

  defp fixture(name) do
    @fixtures |> Path.join(name) |> File.read!() |> Jason.decode!()
  end

  test "request_body/2 reproduces the shared turn_request fixture" do
    expected = fixture("turn_request.json")

    {:ok, envelope} =
      Envelope.from_map(%{
        "engine" => expected["engine"],
        "model" => expected["model"],
        "effort" => expected["effort"],
        "permissions" => expected["permissions"],
        "location" => "local"
      })

    turn = %{
      prompt: expected["prompt"],
      cwd: expected["cwd"],
      tools: expected["tools"],
      run_id: expected["runId"],
      node_id: expected["nodeId"]
    }

    assert {:ok, body} = Client.request_body(envelope, turn)
    # Compare on the wire shape (string keys, JSON scalars), not atom keys.
    assert Jason.decode!(Jason.encode!(body)) == expected
  end

  test "submit_turn maps the shared agent_turn_response fixture's usage to cost" do
    expected = fixture("agent_turn_response.json")
    usage = expected["usage"]

    plug = fn conn ->
      conn
      |> Plug.Conn.put_resp_content_type("application/json")
      |> Plug.Conn.send_resp(200, Jason.encode!(expected))
    end

    {:ok, env} =
      Envelope.from_map(%{"engine" => "codex", "model" => "gpt-5.3-codex", "location" => "local"})

    assert {:ok, %{thread_id: thread_id, event_count: event_count, cost: cost}} =
             Client.submit_turn(env, %{prompt: "hi", cwd: "/w"},
               room_server_url: "http://room.test",
               req_options: [plug: plug]
             )

    assert thread_id == expected["threadId"]
    assert event_count == expected["eventCount"]

    assert cost == %{
             usd: usage["costUsd"],
             tokens_in: usage["tokensIn"],
             tokens_out: usage["tokensOut"],
             cache_read: usage["cacheRead"],
             cache_creation: usage["cacheCreation"]
           }
  end

  test "an unset effort is omitted from the wire shape" do
    {:ok, envelope} =
      Envelope.from_map(%{"engine" => "codex", "model" => "gpt-5.3-codex", "location" => "local"})

    turn = %{prompt: "go", cwd: "/w", tools: [], run_id: "r", node_id: "n"}

    assert {:ok, body} = Client.request_body(envelope, turn)
    refute Map.has_key?(body, "effort")
  end
end
