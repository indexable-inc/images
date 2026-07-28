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

  test "the tool surface is exactly exec" do
    response = Server.handle(request("tools/list", %{}))
    tools = response["result"]["tools"]

    assert tools |> Enum.map(& &1["name"]) == ["exec"]

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

  test "unknown method returns a JSON-RPC error, notifications return nil" do
    assert %{"error" => %{"code" => -32_601}} = Server.handle(request("bogus/method", %{}))
    assert Server.handle(%{"method" => "notifications/initialized"}) == nil
  end

  # -- #3538: binary output must ride the wire, not kill it -------------------

  test "exec of a binary-printing cell replies with escaped output that the wire can encode" do
    response =
      Server.handle(
        request("tools/call", %{
          "name" => "exec",
          "arguments" => %{
            "code" => ~S|IO.puts(<<0xFF>> <> "marker")|,
            "budget" => 2,
            "intent" => "binary output"
          }
        })
      )

    refute response["result"]["isError"]
    [%{"type" => "text", "text" => text}] = response["result"]["content"]
    assert text =~ "\\xFFmarker"

    # The leg that killed the connection: the stdio transport JSON-encodes
    # this exact map, and the OTP encoder raises {:invalid_byte, _} on any
    # invalid UTF-8 anywhere inside it.
    assert is_binary(JSON.encode!(response))
  end

  test "exec output above the cap is truncated loudly, naming the original byte count" do
    total = 300 * 1024

    response =
      Server.handle(
        request("tools/call", %{
          "name" => "exec",
          "arguments" => %{
            "code" => "IO.write(String.duplicate(\"x\", #{total}))",
            "budget" => 5,
            "intent" => "big output"
          }
        })
      )

    [%{"type" => "text", "text" => text}] = response["result"]["content"]
    assert text =~ "output truncated"
    assert text =~ "#{total} bytes"
    # The reply must stay bounded no matter how much the cell printed.
    assert byte_size(text) < 100_000
  end
end
