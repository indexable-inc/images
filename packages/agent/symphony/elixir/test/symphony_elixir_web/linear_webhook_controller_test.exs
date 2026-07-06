defmodule SymphonyElixirWeb.LinearWebhookControllerTest do
  # Swaps the shared Config snapshot in ETS to configure the webhook
  # secret, so this module cannot run concurrently with tests reading the
  # same snapshot.
  use ExUnit.Case, async: false

  import Plug.{Conn, Test}

  @opts SymphonyElixirWeb.Endpoint.init([])
  @secret "linear-controller-secret"

  setup do
    snapshot = SymphonyElixir.Config.get()
    :ets.insert(:symphony_config, {:snapshot, %{snapshot | linear_webhook_secret: @secret}})
    on_exit(fn -> :ets.insert(:symphony_config, {:snapshot, snapshot}) end)
  end

  # A non-Issue event exercises the full endpoint pipeline (RawBodyReader,
  # WebhookAuth over the exact wire bytes, event dispatch) and answers
  # before touching the IR store or the run supervisor.
  test "accepts a correctly signed event" do
    body = Jason.encode!(%{type: "Comment", action: "create"})
    signature = Base.encode16(:crypto.mac(:hmac, :sha256, @secret, body), case: :lower)

    conn =
      :post
      |> conn("/api/v1/triggers/linear", body)
      |> put_req_header("content-type", "application/json")
      |> put_req_header("linear-signature", signature)
      |> SymphonyElixirWeb.Endpoint.call(@opts)

    assert conn.status == 200
    assert Jason.decode!(conn.resp_body) == %{"ok" => true}
  end
end
