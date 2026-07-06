defmodule SymphonyElixirWeb.SlackEventsControllerTest do
  use ExUnit.Case, async: true

  import Plug.Conn
  import Plug.Test

  @opts SymphonyElixirWeb.Endpoint.init([])

  test "rejects Slack events when the signing secret is not configured" do
    conn =
      :post
      |> conn("/api/v1/triggers/slack/events", Jason.encode!(%{type: "event_callback"}))
      |> put_req_header("content-type", "application/json")
      |> SymphonyElixirWeb.Endpoint.call(@opts)

    assert conn.status == 401
    assert Jason.decode!(conn.resp_body) == %{"error" => "slack signing secret not configured"}
  end
end
