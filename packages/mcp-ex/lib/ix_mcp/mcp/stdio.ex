defmodule IxMcp.MCP.Stdio do
  @moduledoc """
  MCP over stdio: newline-delimited JSON-RPC on stdin/stdout. The reader loop
  dispatches every request in its own task, so one slow `exec` call
  never delays the next request -- the same guarantee the jobs layer gives
  cells, applied to the wire.

  All writes to stdout go through this process, which is the only place in
  the application allowed to touch it (logs go to stderr, cell output goes to
  each job's IOProxy).

  Invariant: every request that carries an id gets exactly one reply. A
  handler task that dies is answered with a JSON-RPC error, and a response
  the encoder rejects degrades to one -- silently dropping either left the
  client waiting forever on a connection that looked alive (#3538).
  """

  use GenServer

  alias IxMcp.MCP.ClientRequests
  alias IxMcp.MCP.Notifier
  alias IxMcp.MCP.Server

  require Logger

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
    ClientRequests.register(self())
    reader = self()
    spawn_link(fn -> read_loop(reader) end)
    {:ok, %{pending: %{}, eof: false}}
  end

  @impl true
  def handle_info({:mcp_recv, line}, state) do
    stdio = self()

    # Decode here, not in the task: the reply-always invariant needs the
    # request id BEFORE the handler can die. A crashing task takes
    # everything it knew with it, and an id-less DOWN can only be dropped --
    # exactly the silent hang #3538 diagnosed. Decoding is cheap; the slow
    # part (Server.handle) stays off this process.
    case JSON.decode(line) do
      # An id without a method is the client answering one of OUR requests
      # (elicitation and friends); it gets no reply, so it skips the
      # reply-always pending bookkeeping entirely.
      {:ok, %{"id" => _} = message} when not is_map_key(message, "method") ->
        ClientRequests.resolve(message)
        {:noreply, state}

      {:ok, message} ->
        {:ok, pid} =
          Task.start(fn ->
            case Server.handle(message) do
              nil -> :ok
              response -> send(stdio, {:mcp_send, response})
            end
          end)

        ref = Process.monitor(pid)
        {:noreply, %{state | pending: Map.put(state.pending, ref, Map.get(message, "id"))}}

      {:error, _reason} ->
        write(parse_error())
        {:noreply, state}
    end
  end

  def handle_info({:mcp_send, message}, state) do
    write(message)
    {:noreply, state}
  end

  def handle_info({:DOWN, ref, :process, _pid, reason}, state) do
    {id, pending} = Map.pop(state.pending, ref)

    # A handler task that died abnormally never sent its reply (the send is
    # a finished handler's last act), so this DOWN is the final chance to
    # answer. Dropping the ref without replying -- the pre-#3538 behavior --
    # left the client waiting forever on a request the server had already
    # forgotten. Written before maybe_stop/1 so a shutdown on EOF cannot
    # outrun the reply.
    if reason != :normal and id != nil do
      write(handler_died(id, reason))
    end

    maybe_stop(%{state | pending: pending})
  end

  # The client closed stdin: finish the requests already dispatched, then
  # stop the whole VM cleanly (exit 0) -- an MCP server's life IS its stdin.
  def handle_info(:mcp_eof, state) do
    maybe_stop(%{state | eof: true})
  end

  def handle_info(_msg, state), do: {:noreply, state}

  defp maybe_stop(state) do
    if state.eof and map_size(state.pending) == 0 do
      System.stop(0)
    end

    {:noreply, state}
  end

  defp write(message) do
    # Encoding must not be able to kill the transport: JSON.encode! raises
    # {:invalid_byte, _} on invalid UTF-8, this process owns stdout, and its
    # linked read_loop dies with it -- one bad byte in one response used to
    # cost the whole connection (#3538). Degrade to an error reply for the
    # same id so the client learns the response was unencodable and moves on.
    line =
      try do
        JSON.encode!(message)
      rescue
        error -> JSON.encode!(unencodable(message, error))
      end

    IO.binwrite(:stdio, [line, "\n"])
  end

  defp read_loop(server) do
    case IO.binread(:stdio, :line) do
      :eof ->
        send(server, :mcp_eof)

      {:error, reason} ->
        # A read error is not a clean EOF; name it on stderr before shutting
        # down, or the death is indistinguishable from the client exiting
        # (how #3523 stayed invisible: no log line, exit 0).
        Logger.error("ix-mcp-ex: stdin read error: " <> inspect(reason))
        send(server, :mcp_eof)

      line ->
        case String.trim(line) do
          "" -> :ok
          trimmed -> send(server, {:mcp_recv, trimmed})
        end

        read_loop(server)
    end
  end

  defp handler_died(id, reason) do
    %{
      "jsonrpc" => "2.0",
      "id" => id,
      "error" => %{
        "code" => -32_603,
        # inspect/2 rather than Exception.format_exit/1: inspect's output is
        # always valid UTF-8, and the limits bound a reason that could embed
        # megabytes of crashed-process state.
        "message" => "request handler died: " <> inspect(reason, limit: 25, printable_limit: 500)
      }
    }
  end

  defp unencodable(message, error) do
    %{
      "jsonrpc" => "2.0",
      "id" => Map.get(message, "id"),
      "error" => %{
        "code" => -32_603,
        "message" => "response could not be encoded as JSON: " <> Exception.message(error)
      }
    }
  end

  defp parse_error do
    %{
      "jsonrpc" => "2.0",
      "id" => nil,
      "error" => %{"code" => -32_700, "message" => "parse error"}
    }
  end
end
