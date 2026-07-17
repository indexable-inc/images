defmodule IxMcp.MCP.Stdio do
  @moduledoc """
  MCP over stdio: newline-delimited JSON-RPC on stdin/stdout. The reader loop
  dispatches every request in its own task, so one slow `elixir_exec` call
  never delays the next request -- the same guarantee the jobs layer gives
  cells, applied to the wire.

  All writes to stdout go through this process, which is the only place in
  the application allowed to touch it (logs go to stderr, cell output goes to
  each job's IOProxy).
  """

  use GenServer

  alias IxMcp.MCP.Notifier
  alias IxMcp.MCP.Server

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @impl true
  def init(_) do
    # The wire is bytes: JSON-RPC lines arrive and leave as UTF-8, but the
    # io device defaults to :unicode, where IO.binread/2 fails with
    # {:error, {:no_translation, :unicode, :latin1}} at the first codepoint
    # above 255 -- #3523 (https://github.com/indexable-inc/index/issues/3523):
    # one emoji in a cell payload closed the whole connection. :latin1 +
    # binary makes stdio a transparent byte pipe in both directions.
    :ok = :io.setopts(:standard_io, binary: true, encoding: :latin1)
    Notifier.register(self())
    reader = self()
    spawn_link(fn -> read_loop(reader) end)
    {:ok, %{pending: MapSet.new(), eof: false}}
  end

  @impl true
  def handle_info({:mcp_recv, line}, state) do
    stdio = self()

    {:ok, pid} =
      Task.start(fn ->
        case JSON.decode(line) do
          {:ok, message} ->
            case Server.handle(message) do
              nil -> :ok
              response -> send(stdio, {:mcp_send, response})
            end

          {:error, _reason} ->
            send(stdio, {:mcp_send, parse_error()})
        end
      end)

    ref = Process.monitor(pid)
    {:noreply, %{state | pending: MapSet.put(state.pending, ref)}}
  end

  def handle_info({:mcp_send, message}, state) do
    IO.binwrite(:stdio, [JSON.encode!(message), "\n"])
    {:noreply, state}
  end

  def handle_info({:DOWN, ref, :process, _pid, _reason}, state) do
    maybe_stop(%{state | pending: MapSet.delete(state.pending, ref)})
  end

  # The client closed stdin: finish the requests already dispatched, then
  # stop the whole VM cleanly (exit 0) -- an MCP server's life IS its stdin.
  def handle_info(:mcp_eof, state) do
    maybe_stop(%{state | eof: true})
  end

  def handle_info(_msg, state), do: {:noreply, state}

  defp maybe_stop(state) do
    if state.eof and MapSet.size(state.pending) == 0 do
      System.stop(0)
    end

    {:noreply, state}
  end

  defp read_loop(server) do
    case IO.binread(:stdio, :line) do
      :eof ->
        send(server, :mcp_eof)

      {:error, reason} ->
        # A read error is not a clean EOF; name it on stderr before shutting
        # down, or the death is indistinguishable from the client exiting
        # (how #3523 stayed invisible: no log line, exit 0).
        IO.puts(:stderr, "ix-mcp-ex: stdin read error: " <> inspect(reason))
        send(server, :mcp_eof)

      line ->
        case String.trim(line) do
          "" -> :ok
          trimmed -> send(server, {:mcp_recv, trimmed})
        end

        read_loop(server)
    end
  end

  defp parse_error do
    %{
      "jsonrpc" => "2.0",
      "id" => nil,
      "error" => %{"code" => -32_700, "message" => "parse error"}
    }
  end
end
