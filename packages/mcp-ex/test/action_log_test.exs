defmodule IxMcp.ActionLogTest do
  use ExUnit.Case, async: false

  alias IxMcp.ActionLog
  alias IxMcp.MCP.Server

  test "a tools/call lands one row in the action log" do
    :ok = IxMcp.Session.set_name("action-log-test")
    :ok = IxMcp.Session.set_topic("logging")

    response =
      Server.handle(%{
        "jsonrpc" => "2.0",
        "id" => 1,
        "method" => "tools/call",
        "params" => %{"name" => "kernel_trace", "arguments" => %{}}
      })

    assert %{"result" => %{"isError" => false}} = response

    assert [entry | _rest] = ActionLog.recent(1)
    assert entry.tool == "kernel_trace"
    assert entry.session == "action-log-test"
    assert entry.topic == "logging"
    assert entry.intent == nil
    assert entry.arguments == "{}"
    refute entry.is_error
    assert entry.elapsed_ms >= 0
    assert {:ok, %DateTime{}, 0} = DateTime.from_iso8601(entry.at)
  end

  test "a failing tool call is recorded with is_error and its intent" do
    response =
      Server.handle(%{
        "jsonrpc" => "2.0",
        "id" => 2,
        "method" => "tools/call",
        "params" => %{
          "name" => "elixir_exec",
          "arguments" => %{"intent" => "not really code", "budget" => "bogus"}
        }
      })

    assert %{"result" => %{"isError" => true}} = response

    assert [entry | _rest] = ActionLog.recent(1)
    assert entry.tool == "elixir_exec"
    assert entry.intent == "not really code"
    assert entry.is_error
  end

  test "rows persist in the file across a log restart" do
    path =
      Path.join(
        System.tmp_dir!(),
        "ix-mcp-action-log-test-#{System.unique_integer([:positive])}.db"
      )

    on_exit(fn -> File.rm(path) end)

    log = start_supervised!({ActionLog, path: path, name: :action_log_file_test}, id: :first_open)

    :ok =
      ActionLog.record(
        %{
          session: "s",
          topic: "t",
          tool: "elixir_exec",
          intent: "persist me",
          arguments: "{}",
          is_error: false,
          elapsed_ms: 7
        },
        log
      )

    assert [%{intent: "persist me"}] = ActionLog.recent(20, log)
    stop_supervised!(:first_open)

    reopened =
      start_supervised!({ActionLog, path: path, name: :action_log_reopen_test}, id: :reopen)

    assert [%{intent: "persist me", tool: "elixir_exec"}] = ActionLog.recent(20, reopened)
  end
end
