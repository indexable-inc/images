defmodule IxMcp.Memory.Semantic do
  @moduledoc """
  Owner of the resident `weave recall --stdin` process behind
  `IxMcp.Memory.semantic/2`.

  Loading the embedding model dominates semantic recall (~1.2s for the
  default qwen3-embedding-4b against ~76ms per warm query, #3868), so
  instead of paying it per call the first query starts `weave recall`
  with `--stdin`, which answers its positional query and then keeps the
  model resident, answering one more recall per stdin line with one JSON
  object per line. This server owns that port and serializes queries
  over it.

  The port is keyed by the resolved binary, store, and entry budget: a
  changed WEAVE_BIN / WEAVE_MEMORY_STORE (tests point these at throwaway
  stores) or a limit above the running budget retires the old process
  for a fresh one, and a port whose weave died (killed, store deleted)
  is respawned on the next query instead of wedging the server.
  """

  use GenServer

  alias IxMcp.Memory

  # `--limit` is fixed at spawn (stdin lines carry only query text), so
  # the port over-fetches and `entries/2` truncates; exact brute-force
  # cosine scores every document regardless, so the surplus entries cost
  # only JSON size.
  @entry_budget 64

  # The first answer pays the model load (plus embedding any documents
  # the sidecar is missing); resident answers are sub-second.
  @first_answer_ms 120_000
  @answer_ms 30_000

  @typep state() ::
           %{port: port(), key: {String.t(), String.t(), pos_integer()}, buffer: binary()}
           | nil

  @doc """
  Scored semantic entry points for `query`: up to `limit` maps shaped
  `%{"entity" => id, "similarity" => cosine, "label" => hook-or-nil}`,
  best first, exactly as `weave recall` emitted them.
  """
  @spec entries(String.t(), pos_integer()) :: [map()]
  def entries(query, limit) do
    # Stdin frames one query per line, so multi-line queries flatten.
    query =
      case query |> String.replace(~r/\s+/, " ") |> String.trim() do
        "" -> raise ArgumentError, "semantic recall needs a non-empty query"
        flat -> flat
      end

    case GenServer.call(server(), {:recall, query, limit}, :infinity) do
      {:ok, entries} -> Enum.take(entries, limit)
      {:error, message} -> raise message
    end
  end

  @impl GenServer
  def init(nil), do: {:ok, nil}

  @impl GenServer
  def handle_call({:recall, query, limit}, _from, state) do
    {entries, next} = recall(state, query, limit)
    {:reply, {:ok, entries}, next}
  rescue
    error ->
      close(state)
      {:reply, {:error, Exception.message(error)}, nil}
  end

  # A retired or crashed weave leaves {:data, ...} / {:exit_status, ...}
  # noise behind; dropping it keeps the mailbox from growing forever.
  @impl GenServer
  def handle_info({port, _payload}, state) when is_port(port), do: {:noreply, state}

  # Started lazily and unsupervised on purpose: Memory also works from a
  # plain IEx with no IxMcp application tree running. The race loser
  # adopts the winner's pid.
  @spec server() :: pid()
  defp server do
    case GenServer.start(__MODULE__, nil, name: __MODULE__) do
      {:ok, pid} -> pid
      {:error, {:already_started, pid}} -> pid
    end
  end

  @spec recall(state(), String.t(), pos_integer()) :: {[map()], state()}
  defp recall(nil, query, limit) do
    # Store before binary, matching Memory.run!/1's error priority.
    store = Memory.store!()
    {bin, budget} = {Memory.weave_bin!(), max(limit, @entry_budget)}
    key = {bin, store, budget}

    port =
      Port.open({:spawn_executable, bin}, [
        :binary,
        :exit_status,
        args: [
          "--store",
          store,
          "recall",
          "--limit",
          Integer.to_string(budget),
          "--no-expand",
          "--stdin",
          "--",
          query
        ]
      ])

    try do
      {line, buffer} = read_answer(port, "", @first_answer_ms)
      {decode_entries(line), %{port: port, key: key, buffer: buffer}}
    rescue
      # The port is not in any state yet, so a failed first answer must
      # release it here or the loaded model would linger unreachable.
      error ->
        close(%{port: port})
        reraise error, __STACKTRACE__
    end
  end

  defp recall(%{port: port, key: {bin, store, budget}, buffer: buffer} = state, query, limit) do
    if {store, bin} == {Memory.store!(), Memory.weave_bin!()} and limit <= budget and
         Port.info(port) != nil do
      Port.command(port, query <> "\n")
      {line, rest} = read_answer(port, buffer, @answer_ms)
      {decode_entries(line), %{state | buffer: rest}}
    else
      close(state)
      recall(nil, query, limit)
    end
  end

  @spec read_answer(port(), binary(), pos_integer()) :: {binary(), binary()}
  defp read_answer(port, buffer, timeout) do
    case String.split(buffer, "\n", parts: 2) do
      [line, rest] ->
        {line, rest}

      [_incomplete] ->
        receive do
          {^port, {:data, data}} ->
            read_answer(port, buffer <> data, timeout)

          {^port, {:exit_status, status}} ->
            raise "weave recall exited #{status} before answering: the binary needs " <>
                    "semantic recall (indexable-inc/weave#339+); its stderr went to this console"
        after
          timeout -> raise "weave recall: no answer within #{timeout}ms"
        end
    end
  end

  @spec decode_entries(binary()) :: [map()]
  defp decode_entries(line) do
    case JSON.decode(line) do
      {:ok, %{"entries" => entries}} -> entries
      _ -> raise "weave recall emitted an unexpected answer line: #{inspect(line)}"
    end
  end

  @spec close(state()) :: :ok
  defp close(nil), do: :ok

  defp close(%{port: port}) do
    # Closing stdin is the protocol's EOF: weave exits on it.
    Port.close(port)
    :ok
  catch
    # Already closed (weave exited first): nothing left to release.
    :error, :badarg -> :ok
  end
end
