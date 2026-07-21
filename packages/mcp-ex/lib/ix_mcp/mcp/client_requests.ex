defmodule IxMcp.MCP.ClientRequests do
  @moduledoc """
  Server-initiated JSON-RPC requests to the connected MCP client (the other
  direction from `IxMcp.MCP.Server`): assigns request ids, writes through the
  transport, and parks each caller until the client's response arrives. A
  message from the client that carries an id but no method is a response to
  one of these requests; `IxMcp.MCP.Stdio` routes it here.

  One transport (the Notifier tracks its own list for notifications; requests
  need a reply address, so this server keeps the single live transport and
  fails callers loudly when none is connected). A request that outlives its
  deadline is answered `{:error, :timeout}` and a `notifications/cancelled`
  is sent so the client can tear down whatever UI the request raised.
  """

  use GenServer

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @doc "Adopt a transport; requests write to it as `{:mcp_send, message}`."
  @spec register(pid()) :: :ok
  def register(transport) when is_pid(transport) do
    GenServer.cast(__MODULE__, {:register, transport})
  end

  @doc """
  Send one request and block until the client answers, the deadline passes,
  or the transport dies. Callers own their patience; the GenServer call
  itself never times out.
  """
  @spec request(String.t(), map(), pos_integer()) ::
          {:ok, map()}
          | {:error, :no_transport | :timeout | :transport_closed | {:client_error, map()}}
  def request(method, params, timeout_ms)
      when is_binary(method) and is_map(params) and is_integer(timeout_ms) and timeout_ms > 0 do
    GenServer.call(__MODULE__, {:request, method, params, timeout_ms}, :infinity)
  end

  @doc "Route a client response (id, no method) back to its blocked caller."
  @spec resolve(map()) :: :ok
  def resolve(message) when is_map(message) do
    GenServer.cast(__MODULE__, {:resolve, message})
  end

  @impl true
  def init(_) do
    {:ok, %{transport: nil, next: 1, pending: %{}}}
  end

  @impl true
  def handle_call({:request, _method, _params, _timeout}, _from, %{transport: nil} = state) do
    {:reply, {:error, :no_transport}, state}
  end

  def handle_call({:request, method, params, timeout_ms}, from, state) do
    # A string id namespaced to this side: the client numbers its own
    # requests, and JSON-RPC only disambiguates ids by direction, so a
    # distinct shape makes wire logs unambiguous to human readers too.
    id = "ix-req-" <> Integer.to_string(state.next)

    send(
      state.transport,
      {:mcp_send, %{"jsonrpc" => "2.0", "id" => id, "method" => method, "params" => params}}
    )

    timer = Process.send_after(self(), {:deadline, id}, timeout_ms)
    pending = Map.put(state.pending, id, %{from: from, timer: timer})
    {:noreply, %{state | next: state.next + 1, pending: pending}}
  end

  @impl true
  def handle_cast({:register, transport}, state) do
    Process.monitor(transport)
    {:noreply, %{state | transport: transport}}
  end

  def handle_cast({:resolve, %{"id" => id} = message}, state) do
    case Map.pop(state.pending, id) do
      # A response after its deadline (or a stray id): nothing waits for it.
      {nil, _pending} ->
        {:noreply, state}

      {%{from: from, timer: timer}, pending} ->
        Process.cancel_timer(timer)

        reply =
          case message do
            %{"error" => error} ->
              {:error, {:client_error, error}}

            %{"result" => result} ->
              {:ok, result}

            _ ->
              {:error,
               {:client_error, %{"message" => "response carried neither result nor error"}}}
          end

        GenServer.reply(from, reply)
        {:noreply, %{state | pending: pending}}
    end
  end

  def handle_cast({:resolve, _message}, state), do: {:noreply, state}

  @impl true
  def handle_info({:deadline, id}, state) do
    case Map.pop(state.pending, id) do
      {nil, _pending} ->
        {:noreply, state}

      {%{from: from}, pending} ->
        # Tell the client the request is dead so it can dismiss the dialog;
        # a late answer then lands in the {nil, _} clause above.
        if state.transport do
          send(
            state.transport,
            {:mcp_send,
             %{
               "jsonrpc" => "2.0",
               "method" => "notifications/cancelled",
               "params" => %{"requestId" => id, "reason" => "timed out waiting for an answer"}
             }}
          )
        end

        GenServer.reply(from, {:error, :timeout})
        {:noreply, %{state | pending: pending}}
    end
  end

  def handle_info({:DOWN, _ref, :process, pid, _reason}, %{transport: pid} = state) do
    Enum.each(state.pending, fn {_id, %{from: from, timer: timer}} ->
      Process.cancel_timer(timer)
      GenServer.reply(from, {:error, :transport_closed})
    end)

    {:noreply, %{state | transport: nil, pending: %{}}}
  end

  def handle_info(_msg, state), do: {:noreply, state}
end
