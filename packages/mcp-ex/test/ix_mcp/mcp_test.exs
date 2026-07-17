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

  test "initialize negotiates a version and advertises tools" do
    response = Server.handle(request("initialize", %{"protocolVersion" => "2025-06-18"}))
    assert response["result"]["protocolVersion"] == "2025-06-18"
    assert response["result"]["serverInfo"]["name"] == "ix-mcp-ex"
  end

  test "tools/list exposes elixir_exec with required code+intent" do
    response = Server.handle(request("tools/list", %{}))
    tools = response["result"]["tools"]
    exec = Enum.find(tools, &(&1["name"] == "elixir_exec"))
    assert exec["inputSchema"]["required"] == ["code", "intent"]
  end

  test "tools/call elixir_exec runs code and reports the result" do
    response =
      Server.handle(
        request("tools/call", %{
          "name" => "elixir_exec",
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
    Server.handle(request("tools/call", %{"name" => "session_set_name", "arguments" => %{"name" => "abc"}}))
    Server.handle(request("tools/call", %{"name" => "topic_set", "arguments" => %{"topic" => "xyz"}}))
    assert IxMcp.Session.get() == %{name: "abc", topic: "xyz"}
  end

  test "unknown method returns a JSON-RPC error, notifications return nil" do
    assert %{"error" => %{"code" => -32_601}} = Server.handle(request("bogus/method", %{}))
    assert Server.handle(%{"method" => "notifications/initialized"}) == nil
  end
end
