defmodule SymphonyElixirWeb.RawBodyReader do
  @moduledoc """
  A `Plug.Parsers` body reader that retains the raw request body in
  `conn.assigns.raw_body`.

  Plug.Parsers consumes the request body to decode JSON, which means
  controllers can't recompute an HMAC over the bytes the caller signed.
  Inserting this reader keeps the raw body around so the Linear
  webhook controller can verify `Linear-Signature` before trusting any
  parsed field.

  Only enabled for paths under `/api/v1/triggers/`; other routes pay no
  cost.
  """

  @retain_prefix "/api/v1/triggers/"

  @spec read_body(Plug.Conn.t(), keyword()) :: {:ok, binary(), Plug.Conn.t()} | {:more, binary(), Plug.Conn.t()} | {:error, term()}
  def read_body(conn, opts) do
    case Plug.Conn.read_body(conn, opts) do
      {:ok, body, conn} ->
        {:ok, body, maybe_retain(conn, body)}

      {:more, body, conn} ->
        {:more, body, maybe_retain(conn, body, append: true)}

      {:error, _} = err ->
        err
    end
  end

  defp maybe_retain(conn, body, opts \\ []) do
    if String.starts_with?(conn.request_path, @retain_prefix) do
      Plug.Conn.assign(conn, :raw_body, retained(conn, body, opts))
    else
      conn
    end
  end

  defp retained(conn, body, opts) do
    if Keyword.get(opts, :append, false) do
      (conn.assigns[:raw_body] || "") <> body
    else
      body
    end
  end
end
