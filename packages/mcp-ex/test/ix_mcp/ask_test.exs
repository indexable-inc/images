defmodule IxMcp.AskTest do
  use ExUnit.Case, async: false

  alias IxMcp.Ask
  alias IxMcp.MCP.ClientRequests

  # ClientRequests is a named singleton in the supervision tree; these tests
  # adopt the test process as its transport, so each one drains and answers
  # the wire messages itself. async: false keeps sibling tests from racing
  # for the transport slot.

  setup do
    ClientRequests.register(self())
    :ok
  end

  defp receive_request do
    assert_receive {:mcp_send, %{"id" => id, "method" => method, "params" => params}}, 1_000
    {id, method, params}
  end

  test "Ask.user round trip: options become an enum schema, accept returns the answer" do
    task =
      Task.async(fn ->
        Ask.user("Redesign or patch?", options: ["Redesign", {"Patch", "keep the shape"}])
      end)

    {id, "elicitation/create", params} = receive_request()

    assert params["message"] == "Redesign or patch?"
    answer = params["requestedSchema"]["properties"]["answer"]
    assert answer["enum"] == ["Redesign", "Patch"]
    assert answer["enumNames"] == ["Redesign", "Patch: keep the shape"]
    assert params["requestedSchema"]["required"] == ["answer"]

    ClientRequests.resolve(%{
      "jsonrpc" => "2.0",
      "id" => id,
      "result" => %{"action" => "accept", "content" => %{"answer" => "Redesign"}}
    })

    assert Task.await(task) == {:ok, "Redesign"}
  end

  test "free-text questions carry a plain string schema; decline and cancel map to atoms" do
    for {action, expected} <- [{"decline", :declined}, {"cancel", :cancelled}] do
      task = Task.async(fn -> Ask.user("Name the branch") end)
      {id, "elicitation/create", params} = receive_request()

      answer = params["requestedSchema"]["properties"]["answer"]
      assert answer == %{"type" => "string", "title" => "Answer"}

      ClientRequests.resolve(%{"jsonrpc" => "2.0", "id" => id, "result" => %{"action" => action}})
      assert Task.await(task) == expected
    end
  end

  test "a client error raises with the error in hand" do
    # The raise crosses the async link as an exit signal; trap it so this
    # test observes the exit instead of dying with the task.
    Process.flag(:trap_exit, true)
    task = Task.async(fn -> Ask.user("Anyone home?") end)
    {id, _method, _params} = receive_request()

    ClientRequests.resolve(%{
      "jsonrpc" => "2.0",
      "id" => id,
      "error" => %{"code" => -32_601, "message" => "method not found"}
    })

    assert {{exception, _stack}, _mfa} = catch_exit(Task.await(task))
    assert Exception.message(exception) =~ "method not found"
  end

  test "timeout cancels the dialog and returns :timeout; a late answer is dropped" do
    task = Task.async(fn -> Ask.user("Still there?", timeout_ms: 30) end)
    {id, _method, _params} = receive_request()

    assert Task.await(task) == :timeout

    assert_receive {:mcp_send,
                    %{"method" => "notifications/cancelled", "params" => %{"requestId" => ^id}}},
                   1_000

    # The late answer must not crash the server or leak a reply anywhere.
    ClientRequests.resolve(%{"jsonrpc" => "2.0", "id" => id, "result" => %{"action" => "accept"}})
  end
end
