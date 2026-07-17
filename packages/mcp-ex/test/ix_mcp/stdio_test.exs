defmodule IxMcp.MCP.StdioTest do
  use ExUnit.Case, async: false

  import ExUnit.CaptureIO

  alias IxMcp.MCP.Stdio

  # These drive the GenServer callbacks directly in the test process: the
  # real Stdio owns the actual stdin/stdout (and stops the VM at EOF), so
  # end-to-end wire coverage lives in the package's nix smoke test. What
  # these pin is the reply-always invariant (#3538): every request with an
  # id gets exactly one reply, whatever happens to its handler.

  test "a handler task that dies abnormally still produces an error reply" do
    ref = make_ref()
    state = %{pending: %{ref => 7}, eof: false}
    down = {:DOWN, ref, :process, self(), {:noproc, {GenServer, :call, []}}}

    output =
      capture_io(fn ->
        assert {:noreply, %{pending: pending}} = Stdio.handle_info(down, state)
        assert pending == %{}
      end)

    assert output =~ ~s("id":7)
    assert output =~ "-32603"
    assert output =~ "request handler died"
  end

  test "a handler task that exits :normal already replied; no synthetic error" do
    ref = make_ref()
    state = %{pending: %{ref => 7}, eof: false}

    output =
      capture_io(fn ->
        assert {:noreply, _state} =
                 Stdio.handle_info({:DOWN, ref, :process, self(), :normal}, state)
      end)

    assert output == ""
  end

  test "an unencodable response degrades to an error reply instead of killing the transport" do
    state = %{pending: %{}, eof: false}
    message = %{"jsonrpc" => "2.0", "id" => 3, "result" => %{"content" => <<0xFF>>}}

    output =
      capture_io(fn ->
        assert {:noreply, ^state} = Stdio.handle_info({:mcp_send, message}, state)
      end)

    assert output =~ ~s("id":3)
    assert output =~ "could not be encoded"
  end
end
