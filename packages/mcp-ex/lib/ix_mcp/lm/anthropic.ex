defmodule IxMcp.LM.Anthropic do
  @moduledoc """
  `IxMcp.LM.Backend` over the Anthropic Messages API.

  Transport is OTP's `:httpc`, exactly as `IxMcp.Web` does it (#3798):
  `mix.exs` already ships `:inets` and `:ssl`, so there is no new
  dependency, no `mix.lock` churn and no `pins.json` FOD hash to refresh.
  Elixir has no official Anthropic SDK, so raw HTTP is the sanctioned route
  rather than a shortcut.

  The key comes from `ANTHROPIC_API_KEY` in the kernel's environment and is
  read only at the moment a request is built. A missing key raises, like
  `EXA_API_KEY` and `MOONSHOT_API_KEY` before it: a credential that is not
  configured is a setup error to fix, not a condition to degrade around.

  Non-streaming by design, which bounds `max_tokens` (the provider refuses
  large non-streaming completions). Sub-calls are narrow questions, so this
  is a fit rather than a limitation; streaming would be the change to make
  if `mode: :rlm` ever needs long answers.

  Retries once on the conditions that are worth retrying (429 and 5xx),
  honouring `retry-after`. Everything else is returned as an error for the
  caller to see, because `IxMcp.LM` logs the reason and the budget meter
  needs the truth about what happened.
  """

  @behaviour IxMcp.LM.Backend

  @endpoint "https://api.anthropic.com/v1/messages"
  @api_version "2023-06-01"
  @timeout_ms 120_000
  @retry_status [408, 409, 429, 500, 502, 503, 504, 529]

  @doc """
  One completion. See `IxMcp.LM.Backend.complete/1`.
  """
  @impl IxMcp.LM.Backend
  @spec complete(IxMcp.LM.Backend.request()) ::
          {:ok, IxMcp.LM.Backend.response()} | {:error, term()}
  def complete(request), do: post(body(request), 1)

  @doc false
  @spec api_key() :: String.t()
  def api_key do
    System.get_env("ANTHROPIC_API_KEY") ||
      raise "IxMcp.LM needs ANTHROPIC_API_KEY in the kernel's environment"
  end

  @spec body(IxMcp.LM.Backend.request()) :: map()
  defp body(request) do
    base = %{
      model: request.model,
      max_tokens: request.max_tokens,
      messages: [%{role: "user", content: request.prompt}]
    }

    case Map.get(request, :system) do
      nil -> base
      "" -> base
      system -> Map.put(base, :system, system)
    end
  end

  @spec post(map(), non_neg_integer()) :: {:ok, IxMcp.LM.Backend.response()} | {:error, term()}
  defp post(body, retries_left) do
    request =
      {String.to_charlist(@endpoint),
       [
         {~c"x-api-key", String.to_charlist(api_key())},
         {~c"anthropic-version", String.to_charlist(@api_version)}
       ], ~c"application/json", JSON.encode!(body)}

    case :httpc.request(:post, request, [{:timeout, @timeout_ms}], body_format: :binary) do
      {:ok, {{_version, status, _reason}, _headers, payload}} when status in 200..299 ->
        decode(payload)

      {:ok, {{_version, status, _reason}, headers, payload}}
      when status in @retry_status ->
        if retries_left > 0 do
          Process.sleep(retry_after_ms(headers))
          post(body, retries_left - 1)
        else
          {:error, {:http, status, String.slice(payload, 0, 500)}}
        end

      {:ok, {{_version, status, _reason}, _headers, payload}} ->
        {:error, {:http, status, String.slice(payload, 0, 500)}}

      {:error, reason} ->
        {:error, {:transport, reason}}
    end
  end

  # usage.input_tokens is the UNCACHED remainder only, so a sum that omits the
  # cache fields under-reports what the prompt cost -- which would quietly
  # loosen the budget meter every time prompt caching worked.
  @spec decode(binary()) :: {:ok, IxMcp.LM.Backend.response()} | {:error, term()}
  defp decode(payload) do
    case JSON.decode(payload) do
      {:ok, %{"content" => content, "usage" => usage} = message} ->
        {:ok,
         %{
           text: text(content),
           tokens_in:
             number(usage, "input_tokens") + number(usage, "cache_creation_input_tokens") +
               number(usage, "cache_read_input_tokens"),
           tokens_out: number(usage, "output_tokens"),
           model: Map.get(message, "model", ""),
           stop_reason: Map.get(message, "stop_reason")
         }}

      {:ok, other} ->
        {:error, {:unexpected_response, Map.keys(other)}}

      {:error, reason} ->
        {:error, {:bad_json, reason}}
    end
  end

  @spec text([map()]) :: String.t()
  defp text(content) do
    content
    |> Enum.filter(&(Map.get(&1, "type") == "text"))
    |> Enum.map_join("", &Map.get(&1, "text", ""))
  end

  @spec number(map(), String.t()) :: non_neg_integer()
  defp number(usage, field) do
    case Map.get(usage, field) do
      n when is_integer(n) -> n
      _other -> 0
    end
  end

  @spec retry_after_ms(list()) :: pos_integer()
  defp retry_after_ms(headers) do
    case List.keyfind(headers, ~c"retry-after", 0) do
      {_key, value} ->
        case Integer.parse(to_string(value)) do
          {seconds, _rest} when seconds > 0 and seconds <= 60 -> seconds * 1_000
          _other -> 1_000
        end

      nil ->
        1_000
    end
  end
end
