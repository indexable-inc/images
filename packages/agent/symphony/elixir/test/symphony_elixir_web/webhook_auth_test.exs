defmodule SymphonyElixirWeb.WebhookAuthTest do
  # Each test swaps the shared Config snapshot in ETS (the pattern
  # runtime_test.exs established), so this module cannot run concurrently
  # with tests reading the same snapshot.
  use ExUnit.Case, async: false

  import Plug.Test

  alias SymphonyElixirWeb.WebhookAuth

  @body ~s({"hello":"webhook"})

  defp with_secret(key, value) do
    snapshot = SymphonyElixir.Config.get()
    :ets.insert(:symphony_config, {:snapshot, Map.put(snapshot, key, value)})
    on_exit(fn -> :ets.insert(:symphony_config, {:snapshot, snapshot}) end)
  end

  # Build a conn the way the endpoint hands it to a trigger controller:
  # parsed params are irrelevant here, but the raw body must be assigned
  # exactly as RawBodyReader retains it.
  defp signed_conn(headers, body \\ @body) do
    conn = :post |> conn("/api/v1/triggers/test", body) |> Plug.Conn.assign(:raw_body, body)
    Enum.reduce(headers, conn, fn {name, value}, acc -> Plug.Conn.put_req_header(acc, name, value) end)
  end

  defp hex_hmac(secret, payload) do
    :hmac |> :crypto.mac(:sha256, secret, payload) |> Base.encode16(case: :lower)
  end

  describe "verify/2 for :github" do
    test "accepts a correctly signed body" do
      with_secret(:github_webhook_secret, "gh-secret")
      signature = "sha256=" <> hex_hmac("gh-secret", @body)

      assert WebhookAuth.verify(signed_conn([{"x-hub-signature-256", signature}]), :github) == :ok
    end

    test "rejects a signature computed with the wrong secret" do
      with_secret(:github_webhook_secret, "gh-secret")
      signature = "sha256=" <> hex_hmac("not-the-secret", @body)

      assert WebhookAuth.verify(signed_conn([{"x-hub-signature-256", signature}]), :github) ==
               {:error, :unauthorized, "signature mismatch"}
    end

    test "rejects a truncated signature without raising on the length mismatch" do
      with_secret(:github_webhook_secret, "gh-secret")
      signature = "sha256=" <> String.slice(hex_hmac("gh-secret", @body), 0..-2//1)

      assert WebhookAuth.verify(signed_conn([{"x-hub-signature-256", signature}]), :github) ==
               {:error, :unauthorized, "signature mismatch"}
    end

    test "rejects when the signature header is missing" do
      with_secret(:github_webhook_secret, "gh-secret")

      assert WebhookAuth.verify(signed_conn([]), :github) ==
               {:error, :unauthorized, "missing X-Hub-Signature-256 header"}
    end

    test "rejects when the secret is not configured" do
      with_secret(:github_webhook_secret, nil)
      signature = "sha256=" <> hex_hmac("gh-secret", @body)

      assert WebhookAuth.verify(signed_conn([{"x-hub-signature-256", signature}]), :github) ==
               {:error, :unauthorized, "github webhook secret not configured"}
    end

    test "rejects when the raw body was not retained" do
      with_secret(:github_webhook_secret, "gh-secret")
      signature = "sha256=" <> hex_hmac("gh-secret", @body)

      conn =
        :post
        |> conn("/api/v1/triggers/test", @body)
        |> Plug.Conn.put_req_header("x-hub-signature-256", signature)

      assert WebhookAuth.verify(conn, :github) == {:error, :bad_request, "missing raw body"}
    end
  end

  describe "verify/2 for :slack" do
    @timestamp "1531420618"

    test "accepts a correctly signed body" do
      with_secret(:slack_signing_secret, "slack-secret")
      signature = "v0=" <> hex_hmac("slack-secret", "v0:" <> @timestamp <> ":" <> @body)

      conn = signed_conn([{"x-slack-request-timestamp", @timestamp}, {"x-slack-signature", signature}])
      assert WebhookAuth.verify(conn, :slack) == :ok
    end

    test "rejects when the timestamp differs from the signed one" do
      with_secret(:slack_signing_secret, "slack-secret")
      signature = "v0=" <> hex_hmac("slack-secret", "v0:" <> @timestamp <> ":" <> @body)

      conn = signed_conn([{"x-slack-request-timestamp", "1531420619"}, {"x-slack-signature", signature}])
      assert WebhookAuth.verify(conn, :slack) == {:error, :unauthorized, "signature mismatch"}
    end

    test "rejects when either signature header is missing" do
      with_secret(:slack_signing_secret, "slack-secret")
      signature = "v0=" <> hex_hmac("slack-secret", "v0:" <> @timestamp <> ":" <> @body)

      only_signature = signed_conn([{"x-slack-signature", signature}])
      only_timestamp = signed_conn([{"x-slack-request-timestamp", @timestamp}])

      assert WebhookAuth.verify(only_signature, :slack) ==
               {:error, :unauthorized, "missing Slack signature headers"}

      assert WebhookAuth.verify(only_timestamp, :slack) ==
               {:error, :unauthorized, "missing Slack signature headers"}
    end

    test "rejects when the secret is not configured" do
      with_secret(:slack_signing_secret, nil)
      signature = "v0=" <> hex_hmac("slack-secret", "v0:" <> @timestamp <> ":" <> @body)

      conn = signed_conn([{"x-slack-request-timestamp", @timestamp}, {"x-slack-signature", signature}])

      assert WebhookAuth.verify(conn, :slack) ==
               {:error, :unauthorized, "slack signing secret not configured"}
    end
  end

  describe "verify/2 for :linear" do
    test "accepts a correctly signed body (bare hex digest)" do
      with_secret(:linear_webhook_secret, "linear-secret")
      signature = hex_hmac("linear-secret", @body)

      assert WebhookAuth.verify(signed_conn([{"linear-signature", signature}]), :linear) == :ok
    end

    test "rejects a signature over a different body" do
      with_secret(:linear_webhook_secret, "linear-secret")
      signature = hex_hmac("linear-secret", @body <> "tampered")

      assert WebhookAuth.verify(signed_conn([{"linear-signature", signature}]), :linear) ==
               {:error, :unauthorized, "signature mismatch"}
    end

    test "rejects a wrong-length signature (the guard the old controller skipped)" do
      with_secret(:linear_webhook_secret, "linear-secret")
      signature = String.slice(hex_hmac("linear-secret", @body), 0..-2//1)

      assert WebhookAuth.verify(signed_conn([{"linear-signature", signature}]), :linear) ==
               {:error, :unauthorized, "signature mismatch"}
    end

    test "rejects when the signature header is missing" do
      with_secret(:linear_webhook_secret, "linear-secret")

      assert WebhookAuth.verify(signed_conn([]), :linear) ==
               {:error, :unauthorized, "missing Linear-Signature header"}
    end

    test "rejects when the secret is not configured" do
      with_secret(:linear_webhook_secret, nil)
      signature = hex_hmac("linear-secret", @body)

      assert WebhookAuth.verify(signed_conn([{"linear-signature", signature}]), :linear) ==
               {:error, :unauthorized, "linear webhook secret not configured"}
    end
  end
end
