defmodule IxMcp.MCPTest do
  use ExUnit.Case, async: false

  alias IxMcp.MCP.Server

  setup do
    IxMcp.Workspace.reset()
    :ok
  end

  defp request(method, params, id \\ 1) do
    %{"jsonrpc" => "2.0", "id" => id, "method" => method, "params" => params}
  end

  test "initialize negotiates a version, advertises tools, and teaches the in-language surface" do
    response = Server.handle(request("initialize", %{"protocolVersion" => "2025-06-18"}))
    assert response["result"]["protocolVersion"] == "2025-06-18"
    assert response["result"]["serverInfo"]["name"] == "ix-mcp-ex"
    # The folded-in tools (#3532) arrive as instructions with the connection.
    assert response["result"]["instructions"] =~ "Ix.restart()"
    assert response["result"]["instructions"] =~ "Read.file"
  end

  test "the tool surface is exactly exec, session_set_name, and topic_set" do
    response = Server.handle(request("tools/list", %{}))
    tools = response["result"]["tools"]

    assert tools |> Enum.map(& &1["name"]) |> Enum.sort() ==
             ["exec", "session_set_name", "topic_set"]

    exec = Enum.find(tools, &(&1["name"] == "exec"))
    assert exec["inputSchema"]["required"] == ["code", "intent"]
    # The description carries the in-language surface the removed tools became.
    assert exec["description"] =~ "Ix.trace()"
  end

  test "tools/call exec runs code and reports the result" do
    response =
      Server.handle(
        request("tools/call", %{
          "name" => "exec",
          "arguments" => %{"code" => "IO.puts(\"hi\"); 2 + 2", "intent" => "test add"}
        })
      )

    refute response["result"]["isError"]
    [%{"type" => "text", "text" => text}] = response["result"]["content"]
    assert text =~ ~s("status":"done")
    assert text =~ "hi"
    assert text =~ "=> 4"
  end

  test "session_set_name and topic_set round-trip" do
    Server.handle(
      request("tools/call", %{"name" => "session_set_name", "arguments" => %{"name" => "abc"}})
    )

    Server.handle(
      request("tools/call", %{"name" => "topic_set", "arguments" => %{"topic" => "xyz"}})
    )

    assert IxMcp.Session.get() == %{name: "abc", topic: "xyz"}
  end

  test "unknown method returns a JSON-RPC error, notifications return nil" do
    assert %{"error" => %{"code" => -32_601}} = Server.handle(request("bogus/method", %{}))
    assert Server.handle(%{"method" => "notifications/initialized"}) == nil
  end
end
