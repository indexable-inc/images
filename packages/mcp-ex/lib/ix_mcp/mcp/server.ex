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
          # The experimental "claude/channel" key is what makes this server a
          # channel: Claude Code registers a push listener only when it sees it
          # at initialize, and otherwise drops every notifications/claude/channel
          # event (https://code.claude.com/docs/en/channels-reference, #3785).
          "capabilities" => %{
            "tools" => %{"listChanged" => false},
            "logging" => %{},
            "experimental" => %{"claude/channel" => %{}}
          },
          "serverInfo" => %{"name" => "ix-mcp-ex", "version" => version()},
          "instructions" => instructions()
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
    started = System.monotonic_time(:millisecond)
    action_id = log_start(name, arguments)
    outcome = Tools.call(name, arguments, action_id)

    # exec rows are finalized by the job they spawn (whose eval may outlive
    # this response -- the budget-then-background contract) or by exec's own
    # argument rejection in Tools; every other tool is synchronous, so its
    # wire outcome is its fate. The status='running' guard in finish_action
    # makes a duplicate finalize harmless, never wrong.
    if name != "exec" do
      IxMcp.ActionLog.finish_action(
        action_id,
        finish_status(outcome),
        match?({:error, _}, outcome),
        System.monotonic_time(:millisecond) - started
      )
    end

    case outcome do
      {:ok, text} ->
        result(id, %{"content" => [%{"type" => "text", "text" => text}], "isError" => false})

      {:error, text} ->
        result(id, %{"content" => [%{"type" => "text", "text" => text}], "isError" => true})
    end
  end

  defp handle_tool_call(id, _params), do: error(id, -32_602, "tools/call requires a name")

  # Every tools/call lands one `running` row in the action log BEFORE it
  # executes (#3536), so a reader sees in-flight calls and a crash mid-call
  # leaves a visible running row rather than nothing. Asking the session for
  # ids here -- not at connect time -- is what makes session rows lazy: a
  # connection that never calls a tool leaves no row.
  defp log_start(tool, arguments) do
    %{session_id: session_id, topic_id: topic_id} = IxMcp.Session.ids()

    IxMcp.ActionLog.start_action(%{
      session_id: session_id,
      topic_id: topic_id,
      tool: tool,
      intent: intent(arguments),
      arguments: JSON.encode!(arguments)
    })
  end

  defp finish_status({:ok, _text}), do: "done"
  defp finish_status({:error, _text}), do: "failed"

  # How agents learn the in-language surface (the old read/kernel_trace/
  # kernel_restart/pr_watch/tui_act tools, folded into cells -- #3532):
  # instructions arrive with the connection, so every MCP client sees them.
  defp instructions do
    """
    An MCP server whose REPL is Elixir: `exec` runs cells on one shared,
    persistent workspace (bindings survive across calls), each cell in its
    own supervised BEAM process.

    #{Tools.surface_guide()}
    """
  end

  defp intent(%{"intent" => intent}) when is_binary(intent), do: intent
  defp intent(_arguments), do: nil

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
