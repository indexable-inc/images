defmodule SymphonyElixirWeb.GithubWebhookControllerTest do
  # Swaps the shared Config snapshot in ETS to configure the webhook
  # secret, so this module cannot run concurrently with tests reading the
  # same snapshot.
  use ExUnit.Case, async: false

  import Plug.Conn
  import Plug.Test

  @opts SymphonyElixirWeb.Endpoint.init([])
  @secret "gh-controller-secret"

  setup do
    snapshot = SymphonyElixir.Config.get()
    :ets.insert(:symphony_config, {:snapshot, %{snapshot | github_webhook_secret: @secret}})
    on_exit(fn -> :ets.insert(:symphony_config, {:snapshot, snapshot}) end)
  end

  # A closed PR exercises the full endpoint pipeline (RawBodyReader,
  # WebhookAuth over the exact wire bytes, event handling) and answers
  # before touching the IR store or the run supervisor.
  test "accepts a correctly signed pull_request event" do
    body =
      Jason.encode!(%{
        action: "labeled",
        pull_request: %{number: 7, state: "closed"},
        repository: %{full_name: "acme/widgets"},
        label: %{name: "Deploy"}
      })

    signature = "sha256=" <> Base.encode16(:crypto.mac(:hmac, :sha256, @secret, body), case: :lower)

    conn =
      :post
      |> conn("/api/v1/triggers/github", body)
      |> put_req_header("content-type", "application/json")
      |> put_req_header("x-hub-signature-256", signature)
      |> put_req_header("x-github-event", "pull_request")
      |> SymphonyElixirWeb.Endpoint.call(@opts)

    assert conn.status == 200

    assert Jason.decode!(conn.resp_body) == %{
             "ok" => true,
             "results" => [%{"status" => "ignored", "reason" => "PR is not open"}]
           }
  end
end
