defmodule SymphonyElixirWeb.WebhookAuth do
  @moduledoc """
  Shared HMAC signature verification for the webhook trigger controllers.

  Each provider signs the raw request bytes (preserved by
  `SymphonyElixirWeb.RawBodyReader`) with a shared secret; a controller
  calls `verify/2` before trusting any parsed field. Keeping the check here
  pins the semantics every provider must share: an unconfigured secret
  fails closed with 401 (an empty-secret deployment must not silently
  accept unsigned traffic), a missing raw body is a 400 (the request never
  went through the body reader), and every comparison is a length-guarded
  `Plug.Crypto.secure_compare`.

  Only the signature scheme differs per provider:

  - `:github` signs the raw body; `X-Hub-Signature-256` carries
    `"sha256=" <> hex`.
  - `:slack` signs `"v0:" <> timestamp <> ":" <> body` where the timestamp
    rides in `X-Slack-Request-Timestamp`; `X-Slack-Signature` carries
    `"v0=" <> hex`.
  - `:linear` signs the raw body; `Linear-Signature` carries the bare hex
    digest.
  """

  alias SymphonyElixir.Config

  @typedoc "A webhook provider with a configured signing secret."
  @type provider :: :github | :slack | :linear

  @doc """
  Verify the request's HMAC signature against `provider`'s configured
  secret. Returns `:ok` or `{:error, status, message}` for the controller
  to render.
  """
  @spec verify(Plug.Conn.t(), provider()) :: :ok | {:error, :unauthorized | :bad_request, String.t()}
  def verify(conn, :github) do
    with {:ok, secret} <- secret(Config.get().github_webhook_secret, "github webhook secret not configured"),
         {:ok, body} <- raw_body(conn),
         {:ok, provided} <- header(conn, "x-hub-signature-256", "missing X-Hub-Signature-256 header") do
      compare(provided, "sha256=" <> hex_hmac(secret, body))
    end
  end

  def verify(conn, :slack) do
    with {:ok, secret} <- secret(Config.get().slack_signing_secret, "slack signing secret not configured"),
         {:ok, body} <- raw_body(conn),
         {:ok, timestamp} <- header(conn, "x-slack-request-timestamp", "missing Slack signature headers"),
         {:ok, provided} <- header(conn, "x-slack-signature", "missing Slack signature headers") do
      compare(provided, "v0=" <> hex_hmac(secret, "v0:" <> timestamp <> ":" <> body))
    end
  end

  def verify(conn, :linear) do
    with {:ok, secret} <- secret(Config.get().linear_webhook_secret, "linear webhook secret not configured"),
         {:ok, body} <- raw_body(conn),
         {:ok, provided} <- header(conn, "linear-signature", "missing Linear-Signature header") do
      compare(provided, hex_hmac(secret, body))
    end
  end

  # An absent secret refuses every request rather than skipping the check,
  # so a deployment missing the env var cannot accept unsigned traffic.
  defp secret(nil, message), do: {:error, :unauthorized, message}
  defp secret(secret, _message), do: {:ok, secret}

  # Verification needs the exact bytes the provider signed; only requests
  # routed through RawBodyReader (the /api/v1/triggers/ paths) carry them.
  defp raw_body(conn) do
    case conn.assigns[:raw_body] do
      nil -> {:error, :bad_request, "missing raw body"}
      body -> {:ok, body}
    end
  end

  defp header(conn, name, message) do
    case conn |> Plug.Conn.get_req_header(name) |> List.first() do
      nil -> {:error, :unauthorized, message}
      value -> {:ok, value}
    end
  end

  defp hex_hmac(secret, payload) do
    :hmac
    |> :crypto.mac(:sha256, secret, payload)
    |> Base.encode16(case: :lower)
  end

  # secure_compare is only constant-time over equal-length inputs, so guard
  # the length first; unequal lengths can never match anyway.
  defp compare(provided, expected) do
    if byte_size(provided) == byte_size(expected) and Plug.Crypto.secure_compare(provided, expected) do
      :ok
    else
      {:error, :unauthorized, "signature mismatch"}
    end
  end
end
