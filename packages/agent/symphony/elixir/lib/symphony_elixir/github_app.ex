defmodule SymphonyElixir.GithubApp do
  @moduledoc """
  Mint and cache installation access tokens for a configured GitHub App.

  Why this exists:

  - PRs authored under an installation token show up under the App's
    bot identity (`<app-slug>[bot]`) at the API level. Workflow packs
    that gate behavior on a specific bot author (e.g. auto-merge guards)
    rely on this. PRs authored with a human PAT bypass that gate.
  - Doing the JWT signature in Elixir via `:public_key.sign/4` keeps
    token minting in-process with no `openssl` dependency on the host.
  - Centralizing the mint in one process lets us cache the installation
    token across nodes in the same run (and across runs in the same
    hour) instead of paying the JWT + REST round-trip on every node.

  Token lifecycle:

  - Build a JWT signed with the App's RSA private key (RS256). `iat`
    is 60s in the past for clock skew tolerance; `exp` is 9 minutes out
    (GitHub caps JWT exp at 10 minutes).
  - POST that JWT to `/app/installations/<id>/access_tokens` to receive
    the installation token. GitHub installation tokens expire after one
    hour.
  - Cache the token until `expires_at - skew`. The skew must be long
    enough for a Codex node to finish implementation and still push with
    the token stamped into the workspace's git config.

  This module is a no-op when `SYMPHONY_GITHUB_APP_ID` is unset. That is
  the dev-laptop default; production hosts must set the App credentials.
  Callers that require the bot identity should match on
  `{:error, :not_configured}` and decide whether to fail or proceed
  with the ambient `GITHUB_TOKEN`.
  """

  use GenServer

  alias SymphonyElixir.Config

  require Logger

  @table :symphony_github_app_token

  # Re-mint when the cached token has less than this much life left.
  # GitHub installation tokens last 60 minutes. Skill nodes routinely spend
  # 10-15 minutes before their final git push, and the token is copied into
  # the workspace's git config only once at node startup. Keep enough
  # headroom that a token accepted at startup is still valid at publish time.
  @reissue_skew_seconds 30 * 60

  # GitHub JWT max exp is 10 minutes from iat. Use 9 to leave a safety
  # margin against clock skew between this host and api.github.com.
  @jwt_lifetime_seconds 9 * 60
  @jwt_clock_skew_seconds 60

  @type token :: %{token: String.t(), expires_at: DateTime.t(), installation_id: integer()}

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Return a valid installation token, minting one if the cache is cold
  or near expiry. `{:error, :not_configured}` when the App credentials
  are absent (dev laptops); `{:error, reason}` on mint failure.
  """
  @spec installation_token() :: {:ok, String.t()} | {:error, term()}
  def installation_token do
    case :ets.lookup(@table, :current) do
      [{:current, %{token: token, expires_at: expires_at}}] ->
        if DateTime.diff(expires_at, DateTime.utc_now()) > @reissue_skew_seconds do
          {:ok, token}
        else
          GenServer.call(__MODULE__, :mint, 30_000)
        end

      [] ->
        GenServer.call(__MODULE__, :mint, 30_000)
    end
  end

  @doc """
  True iff `SYMPHONY_GITHUB_APP_ID` and the private key are both configured.
  Used by the IR exec runner to decide whether to attempt token injection
  at all.
  """
  @spec configured?() :: boolean()
  def configured? do
    configured?(Config.get())
  end

  @doc """
  Pure variant: caller passes the config snapshot. Lets tests probe
  this decision without needing the Config GenServer running.
  """
  @spec configured?(Config.t() | map()) :: boolean()
  def configured?(%{github_app_id: id, github_app_private_key_pem: pem}) do
    is_binary(id) and id != "" and is_binary(pem) and pem != ""
  end

  # astlog-ignore: public-def-needs-spec
  def configured?(_), do: false

  @impl true
  def init(_opts) do
    :ets.new(@table, [:named_table, :public, read_concurrency: true])
    {:ok, %{}}
  end

  @impl true
  def handle_call(:mint, _from, state) do
    # Double-check cache after acquiring the call lock; a concurrent
    # caller may have minted between our ETS read and this point.
    case :ets.lookup(@table, :current) do
      [{:current, %{token: token, expires_at: expires_at}}] ->
        if DateTime.diff(expires_at, DateTime.utc_now()) > @reissue_skew_seconds do
          {:reply, {:ok, token}, state}
        else
          do_mint(state)
        end

      [] ->
        do_mint(state)
    end
  end

  defp do_mint(state) do
    case mint_token() do
      {:ok, %{token: token} = entry} ->
        :ets.insert(@table, {:current, entry})
        {:reply, {:ok, token}, state}

      {:error, reason} = err ->
        Logger.warning("GithubApp mint failed: #{inspect(reason)}")
        {:reply, err, state}
    end
  end

  @spec mint_token() :: {:ok, token()} | {:error, term()}
  defp mint_token do
    config = Config.get()

    with :ok <- ensure_configured(config),
         :ok <- ensure_owner_repo(config),
         {:ok, pem} <- decode_pem(config.github_app_private_key_pem),
         {:ok, jwt} <- build_jwt(config.github_app_id, pem),
         {:ok, installation_id} <- fetch_installation_id(jwt, config.github_app_owner_repo),
         {:ok, body} <- request_installation_token(jwt, installation_id) do
      parse_token_response(body, installation_id)
    end
  end

  defp ensure_configured(%Config{github_app_id: id, github_app_private_key_pem: pem})
       when is_binary(id) and is_binary(pem),
       do: :ok

  defp ensure_configured(_), do: {:error, :not_configured}

  defp ensure_owner_repo(%Config{github_app_owner_repo: repo}) when is_binary(repo) and repo != "",
    do: :ok

  defp ensure_owner_repo(_), do: {:error, :missing_owner_repo}

  defp decode_pem(pem) when is_binary(pem) do
    case :public_key.pem_decode(pem) do
      [entry | _] ->
        try do
          {:ok, :public_key.pem_entry_decode(entry)}
        rescue
          e -> {:error, {:pem_decode_failed, Exception.message(e)}}
        end

      [] ->
        {:error, :pem_empty}
    end
  end

  defp build_jwt(app_id, private_key) when is_binary(app_id) do
    now = System.system_time(:second)

    header = %{"alg" => "RS256", "typ" => "JWT"}

    claims = %{
      "iat" => now - @jwt_clock_skew_seconds,
      "exp" => now + @jwt_lifetime_seconds,
      "iss" => app_id
    }

    with {:ok, header_b64} <- encode_segment(header),
         {:ok, claims_b64} <- encode_segment(claims) do
      signing_input = header_b64 <> "." <> claims_b64
      signature = :public_key.sign(signing_input, :sha256, private_key)
      signature_b64 = Base.url_encode64(signature, padding: false)
      {:ok, signing_input <> "." <> signature_b64}
    end
  end

  defp encode_segment(map) do
    case Jason.encode(map) do
      {:ok, json} -> {:ok, Base.url_encode64(json, padding: false)}
      {:error, reason} -> {:error, {:json_encode_failed, reason}}
    end
  end

  defp fetch_installation_id(jwt, owner_repo) when is_binary(owner_repo) do
    url = "https://api.github.com/repos/" <> owner_repo <> "/installation"

    case Req.get(url, headers: gh_headers(jwt)) do
      {:ok, %{status: 200, body: %{"id" => id}}} when is_integer(id) ->
        {:ok, id}

      {:ok, %{status: status, body: body}} ->
        {:error, {:installation_lookup_failed, status, body}}

      {:error, reason} ->
        {:error, {:installation_lookup_transport, reason}}
    end
  end

  defp request_installation_token(jwt, installation_id) do
    url = "https://api.github.com/app/installations/#{installation_id}/access_tokens"

    case Req.post(url, headers: gh_headers(jwt), body: "") do
      {:ok, %{status: 201, body: body}} when is_map(body) ->
        {:ok, body}

      {:ok, %{status: status, body: body}} ->
        {:error, {:token_mint_failed, status, body}}

      {:error, reason} ->
        {:error, {:token_mint_transport, reason}}
    end
  end

  defp parse_token_response(%{"token" => token, "expires_at" => expires_at_iso}, installation_id)
       when is_binary(token) and is_binary(expires_at_iso) do
    case DateTime.from_iso8601(expires_at_iso) do
      {:ok, expires_at, _offset} ->
        {:ok, %{token: token, expires_at: expires_at, installation_id: installation_id}}

      {:error, reason} ->
        {:error, {:invalid_expires_at, reason}}
    end
  end

  defp parse_token_response(other, _id), do: {:error, {:malformed_token_payload, other}}

  defp gh_headers(jwt) do
    [
      {"accept", "application/vnd.github+json"},
      {"authorization", "Bearer " <> jwt},
      {"x-github-api-version", "2022-11-28"},
      {"user-agent", "symphony-github-app/0.2.0"}
    ]
  end
end
