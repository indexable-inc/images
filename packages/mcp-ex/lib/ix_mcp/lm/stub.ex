defmodule IxMcp.LM.Stub do
  @moduledoc """
  A deterministic `IxMcp.LM.Backend` that answers without a network.

  Two jobs. In tests it is the substitute that lets the parts worth testing
  -- memoization, budget refusal, event-log shape -- be tested exactly, with
  a counter proving how many calls actually reached a provider. In an exec
  cell it is a dry run: `config :ix_mcp, :lm_backend, IxMcp.LM.Stub` and a
  fan-out can be shaped and costed before a key is ever needed.

  The answer is `config :ix_mcp, :lm_stub, fn request -> ... end` (returning
  a string, or a full `{:ok, response}` / `{:error, reason}`); the default
  echoes a digest of the request, which is enough to prove a cache hit
  returned the SAME answer rather than a fresh one.
  """

  @behaviour IxMcp.LM.Backend

  @counter :ix_mcp_lm_stub_calls

  @doc "See `IxMcp.LM.Backend.complete/1`."
  @impl IxMcp.LM.Backend
  @spec complete(IxMcp.LM.Backend.request()) ::
          {:ok, IxMcp.LM.Backend.response()} | {:error, term()}
  def complete(request) do
    bump()

    case Application.get_env(:ix_mcp, :lm_stub) do
      nil -> {:ok, response(request, default_text(request))}
      fun when is_function(fun, 1) -> normalize(fun.(request), request)
    end
  end

  @doc """
  How many completions have actually been performed since `reset/0`.

  This is the instrument that makes a memoization test mean something: a
  cache hit must leave this number unchanged.
  """
  @spec calls() :: non_neg_integer()
  def calls do
    case :persistent_term.get({__MODULE__, @counter}, nil) do
      nil -> 0
      counters -> :counters.get(counters, 1)
    end
  end

  @doc "Zero the call counter."
  @spec reset() :: :ok
  def reset do
    :counters.put(counters(), 1, 0)
    :ok
  end

  @spec normalize(term(), IxMcp.LM.Backend.request()) ::
          {:ok, IxMcp.LM.Backend.response()} | {:error, term()}
  defp normalize({:ok, response}, _request), do: {:ok, response}
  defp normalize({:error, reason}, _request), do: {:error, reason}
  defp normalize(text, request) when is_binary(text), do: {:ok, response(request, text)}

  @spec response(IxMcp.LM.Backend.request(), String.t()) :: IxMcp.LM.Backend.response()
  defp response(request, text) do
    %{
      text: text,
      tokens_in: div(byte_size(request.prompt), 4),
      tokens_out: div(byte_size(text), 4),
      model: request.model,
      stop_reason: "end_turn"
    }
  end

  @spec default_text(IxMcp.LM.Backend.request()) :: String.t()
  defp default_text(request) do
    "stub(#{request.model}): " <> String.slice(IxMcp.Blake3.hash_hex(request.prompt), 0, 16)
  end

  @spec bump() :: :ok
  defp bump do
    :counters.add(counters(), 1, 1)
    :ok
  end

  @spec counters() :: :counters.counters_ref()
  defp counters do
    case :persistent_term.get({__MODULE__, @counter}, nil) do
      nil ->
        counters = :counters.new(1, [:write_concurrency])
        :persistent_term.put({__MODULE__, @counter}, counters)
        counters

      counters ->
        counters
    end
  end
end
