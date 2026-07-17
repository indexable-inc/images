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

  alias IxMcp.MCP.Server

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @impl true
  def init(_) do
    IxMcp.MCP.Notifier.register(self())
    reader = self()
    spawn_link(fn -> read_loop(reader) end)
    {:ok, %{}}
  end

  @impl true
  def handle_info({:mcp_recv, line}, state) do
    stdio = self()

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

    {:noreply, state}
  end

  def handle_info({:mcp_send, message}, state) do
    IO.binwrite(:stdio, [JSON.encode!(message), "\n"])
    {:noreply, state}
  end

  def handle_info(:mcp_eof, state) do
    {:stop, :normal, state}
  end

  def handle_info(_msg, state), do: {:noreply, state}

  defp read_loop(server) do
    case IO.binread(:stdio, :line) do
      :eof ->
        send(server, :mcp_eof)

      {:error, _reason} ->
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
    %{"jsonrpc" => "2.0", "id" => nil, "error" => %{"code" => -32_700, "message" => "parse error"}}
  end
end
