defmodule IxMcp.Web do
  @moduledoc """
  Web search and fetch, in-language.

  The kernel's answer to the two things an agent most obviously loses when the
  session runs kernel-only (`kernelOnly` in the agent policy): a search tool and
  a fetch tool. Both come back here as ordinary Elixir calls against the same
  Exa API the `exa` MCP server was talking to, so the capability is unchanged
  and only the door moves. Aliased in every cell as `Web`.

  Being in-language rather than a tool call is the actual win. A search result
  is a list of maps, so it composes: filter it, `Enum.map` it into `fetch/2`,
  keep it in a binding and come back to it three cells later. A tool call hands
  the model prose it has to re-read every time.

      Web.search("erlang httpc streaming body")
      Web.search("nix ca-derivations", results: 3, text: false)
      Web.fetch("https://www.erlang.org/doc/apps/inets/httpc.html")
      urls |> Enum.map(&Web.fetch(&1, chars: 2_000))

  Transport is OTP's `:httpc`, already available because `mix.exs` ships
  `:inets` and `:ssl` for exactly this (#3798). No new dependency, so no
  `mix.lock` churn and no `pins.json` FOD hash to refresh.

  Needs `EXA_API_KEY` in the kernel's environment. Absent, every call raises
  and says so, following `MOONSHOT_API_KEY` in `IxMcp.Agents.Backend`: a
  credential that is missing is a setup error to fix, not a condition to
  degrade around.
  """

  @endpoint "https://api.exa.ai"
  @default_results 8
  @default_chars 12_000
  @timeout_ms 30_000

  @typedoc "One search hit or fetched document."
  @type result :: %{
          url: String.t(),
          title: String.t() | nil,
          author: String.t() | nil,
          published: String.t() | nil,
          text: String.t() | nil
        }

  @doc """
  Search the web. Returns hits newest-relevance first, each a `t:result/0`.

  Options:

    * `:results` - how many hits (default #{@default_results})
    * `:text` - include page text on each hit (default `true`). `false` is much
      cheaper when you only want URLs to feed `fetch/2`.
    * `:chars` - per-hit text cap (default #{@default_chars}, `:all` to lift)
  """
  @spec search(String.t(), keyword()) :: [result()]
  def search(query, opts \\ []) when is_binary(query) do
    "/search"
    |> post(search_body(query, opts))
    |> parse_results(opts)
  end

  @doc """
  Fetch one URL as clean text, or a list of URLs as a list of `t:result/0`.

  A single URL returns the text directly, because that is what a cell almost
  always wants to pipe onward. Options are `:chars` as in `search/2`.
  """
  @spec fetch(String.t() | [String.t()], keyword()) :: String.t() | [result()]
  def fetch(url, opts \\ [])

  def fetch(url, opts) when is_binary(url) do
    case fetch([url], opts) do
      [%{text: text}] when is_binary(text) -> text
      _ -> raise "Web.fetch: no contents returned for #{url}"
    end
  end

  def fetch(urls, opts) when is_list(urls) do
    "/contents"
    |> post(contents_body(urls))
    |> parse_results(opts)
  end

  # --- request bodies (pure) ---

  @doc false
  @spec search_body(String.t(), keyword()) :: map()
  def search_body(query, opts) do
    with_text = Keyword.get(opts, :text, true)

    %{
      "query" => query,
      "numResults" => Keyword.get(opts, :results, @default_results),
      "type" => "auto",
      "contents" => %{"text" => with_text}
    }
  end

  @doc false
  @spec contents_body([String.t()]) :: map()
  def contents_body(urls), do: %{"urls" => urls, "text" => true}

  # --- response shaping (pure) ---

  @doc false
  @spec parse_results(term(), keyword()) :: [result()]
  def parse_results(%{"results" => results}, opts) when is_list(results) do
    Enum.map(results, &normalize(&1, opts))
  end

  def parse_results(other, _opts) do
    raise "Web: unexpected response shape, no \"results\" key: #{inspect(other, limit: 5)}"
  end

  defp normalize(hit, opts) when is_map(hit) do
    %{
      url: Map.get(hit, "url"),
      title: Map.get(hit, "title"),
      author: Map.get(hit, "author"),
      published: Map.get(hit, "publishedDate"),
      text: clip(Map.get(hit, "text"), Keyword.get(opts, :chars, @default_chars))
    }
  end

  @doc false
  @spec clip(String.t() | nil, pos_integer() | :all) :: String.t() | nil
  def clip(nil, _cap), do: nil
  def clip(text, :all), do: text

  def clip(text, cap) when is_binary(text) and is_integer(cap) do
    if String.length(text) <= cap do
      text
    else
      # Say that it was cut, and name the option that un-cuts it. A silent
      # truncation reads downstream as a page that simply ended.
      String.slice(text, 0, cap) <>
        "\n\n[Web: truncated at #{cap} chars; pass chars: :all for the whole document]"
    end
  end

  # --- transport ---

  @doc false
  @spec api_key() :: String.t()
  def api_key do
    System.get_env("EXA_API_KEY") ||
      raise "Web.search/Web.fetch need EXA_API_KEY in the kernel's environment"
  end

  defp post(path, body) do
    request =
      {String.to_charlist(@endpoint <> path), [{~c"x-api-key", String.to_charlist(api_key())}],
       ~c"application/json", JSON.encode!(body)}

    case :httpc.request(:post, request, [{:timeout, @timeout_ms}], body_format: :binary) do
      {:ok, {{_version, status, _reason}, _headers, payload}} when status in 200..299 ->
        decode(payload)

      {:ok, {{_version, status, _reason}, _headers, payload}} ->
        raise "Web: exa returned HTTP #{status}: #{String.slice(to_string(payload), 0, 500)}"

      {:error, reason} ->
        raise "Web: request to #{path} failed: #{inspect(reason)}"
    end
  end

  defp decode(payload) do
    case JSON.decode(payload) do
      {:ok, decoded} -> decoded
      {:error, reason} -> raise "Web: exa returned undecodable JSON: #{inspect(reason)}"
    end
  end
end
