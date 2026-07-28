defmodule GmailExTest do
  use ExUnit.Case, async: true

  # The sandbox has no client credentials and no stored grant, so the suite
  # proves the offline half of the boundary: the NIF loads, records decode,
  # local input validation fires before any network, and a signed-out call
  # is a typed error rather than a crash. The online half (a real send) is
  # a workstation concern, not a build gate.

  @required_scopes [
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/gmail.send"
  ]

  test "status reports the signed-out state as data, not an error" do
    status = GmailEx.status()
    assert %GmailEx.AuthStatus{} = status
    assert status.configured == false
    assert status.signed_in == false
    assert status.scopes == []
    assert Enum.sort(status.missing_scopes) == @required_scopes
  end

  test "send with no usable recipient is rejected locally as :bad_input" do
    assert {:error, %GmailEx.GmailError{variant: :bad_input, message: message}} =
             GmailEx.send(" , ", "subject", "body")

    assert message =~ "recipient"
  end

  test "send without credentials surfaces the :auth variant" do
    assert {:error, %GmailEx.GmailError{variant: :auth, message: message}} =
             GmailEx.send("someone@example.com", "subject", "body")

    assert message =~ "GOOGLE_OAUTH_CLIENT_ID"
  end
end
