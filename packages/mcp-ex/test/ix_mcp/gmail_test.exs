defmodule IxMcp.GmailTest do
  use ExUnit.Case, async: true

  alias IxMcp.Gmail

  @moduletag :gmail_ex

  # What these defend: the runtime load of the :gmail_ex app (code path,
  # app load, NIF @on_load) and the facade's argument plumbing into the
  # generated bindings. The Gmail behavior itself is covered by the
  # binding's own suite (checks.*.google-gmail-ex-run); nothing here needs
  # credentials or network.

  test "loads the NIF app and answers status as data" do
    assert %{__struct__: _, signed_in: signed_in} = Gmail.status()
    assert is_boolean(signed_in)
  end

  test "a send with no usable recipient is a typed error, not a crash" do
    assert {:error, %{variant: :bad_input}} = Gmail.send("", "subject", "body")
  end
end
