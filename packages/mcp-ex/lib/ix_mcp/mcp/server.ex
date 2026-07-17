defmodule IxMcp.MCP.Server do
  @moduledoc """
  Transport-independent MCP request handling: takes one decoded JSON-RPC
  message, returns the response map (or `nil` for notifications). The tool
  surface itself lives in `IxMcp.MCP.Tools`.
  """

  alias IxMcp.MCP.Tools

  @protocol_version "2025-06-18"

  @spec handle(map()) :: map() | nil
  def handle(%{"method" => method} = message) do
    id = Map.get(message, "id")
    params = Map.get(message, "params", %{})

    case {method, id} do
      {"initialize", id} when id != nil ->
        result(id, %{
          "protocolVersion" => negotiate_version(params),
          "capabilities" => %{"tools" => %{"listChanged" => false}, "logging" => %{}},
          "serverInfo" => %{"name" => "ix-mcp-ex", "version" => version()}
        })

      {"ping", id} when id != nil ->
        result(id, %{})

      {"tools/list", id} when id != nil ->
        result(id, %{"tools" => Tools.list()})

      {"tools/call", id} when id != nil ->
        handle_tool_call(id, params)

      {_notification, nil} ->
        nil

      {other, id} ->
        error(id, -32_601, "method not found: #{other}")
    end
  end

  def handle(_message), do: error(nil, -32_600, "invalid request")

  defp handle_tool_call(id, %{"name" => name} = params) do
    arguments = Map.get(params, "arguments", %{})

    case Tools.call(name, arguments) do
      {:ok, text} ->
        result(id, %{"content" => [%{"type" => "text", "text" => text}], "isError" => false})

      {:error, text} ->
        result(id, %{"content" => [%{"type" => "text", "text" => text}], "isError" => true})
    end
  end

  defp handle_tool_call(id, _params), do: error(id, -32_602, "tools/call requires a name")

  defp result(id, result), do: %{"jsonrpc" => "2.0", "id" => id, "result" => result}

  defp error(id, code, message),
    do: %{"jsonrpc" => "2.0", "id" => id, "error" => %{"code" => code, "message" => message}}

  # Echo the client's version when we can speak it; otherwise offer ours.
  defp negotiate_version(%{"protocolVersion" => v}) when is_binary(v), do: v
  defp negotiate_version(_), do: @protocol_version

  defp version do
    case :application.get_key(:ix_mcp, :vsn) do
      {:ok, vsn} -> List.to_string(vsn)
      _ -> "0.0.0"
    end
  end
end
