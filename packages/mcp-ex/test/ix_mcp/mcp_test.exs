defmodule IxMcp.MCPTest do
  use ExUnit.Case, async: false

  alias IxMcp.MCP.Server
  alias IxMcp.MCP.Tools

  setup do
    IxMcp.Workspace.reset()
    :ok
  end

  defp request(method, params, id \\ 1) do
    %{"jsonrpc" => "2.0", "id" => id, "method" => method, "params" => params}
  end

  # The guide is rendered into a client that TRUNCATES it, so its line budget
  # is part of its contract. Claude Code cuts MCP instructions around 50 lines;
  # a capability below the cut does not exist for the reader, and one session
  # spent an hour hand-rolling System.cmd because Agents sat at line 236.
  # Two-sided on purpose: it fails if Agents drifts below the budget AND if it
  # disappears from the guide altogether.
  test "the exec guide names the fan-out surface inside the client's truncation budget" do
    lines = String.split(Tools.surface_guide(), "\n")

    agents_at = Enum.find_index(lines, &String.contains?(&1, "Agents.spawn("))
    assert agents_at, "the guide no longer names Agents.spawn at all"

    assert agents_at < 40,
           "Agents.spawn is at guide line #{agents_at + 1}, past the 40-line budget " <>
             "(the client renders about 50 lines)"

    # The other two a session reaches for constantly must clear the cut too.
    for call <- ["Cmd.run(", "Edit.replace("] do
      at = Enum.find_index(lines, &String.contains?(&1, call))
      assert at, "the guide no longer names #{call}"
      assert at < 50, "#{call} is at guide line #{at + 1}, past the rendered budget"
    end
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
