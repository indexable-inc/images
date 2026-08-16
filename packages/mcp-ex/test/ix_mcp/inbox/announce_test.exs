defmodule IxMcp.Inbox.AnnounceTest do
  use ExUnit.Case, async: false

  alias IxMcp.Inbox.Announce
  alias IxMcp.MCP.Notifier

  # Every name here is invented. This tree is public, so no test may carry a
  # real contact, handle, address, or chat title. The source label is a
  # fixture too: a leaked global feed (IX_MCP_STDIO=1 reaches `mix test` from
  # a kernel-launched shell) announces with source "beeper", and an assertion
  # matching that could pass on a real message.
  @source "fixture"
  @item %{
    id: "fixture-msg-1",
    platform: "Signal",
    sender: "Fixture Sender",
    context: "Fixture Group",
    preview: "hello there"
  }

  setup do
    Notifier.register(self())
    # register/1 is a cast; sync on the Notifier so an announce below cannot
    # be processed ahead of the registration.
    _state = :sys.get_state(Notifier)
    :ok
  end

  defp await_channel do
    assert_receive {:mcp_send,
                    %{
                      "method" => "notifications/claude/channel",
                      "params" => %{"meta" => %{"source" => @source}} = params
                    }},
                   2_000

    params
  end

  test "one message becomes one channel line, with string-only meta" do
    Announce.announce(@source, @item)

    assert %{"content" => content, "meta" => meta} = await_channel()
    assert content == "Signal - Fixture Sender (Fixture Group): hello there"
    assert meta["platform"] == "Signal"
    assert meta["sender"] == "Fixture Sender"
    assert meta["id"] == "fixture-msg-1"

    # The client parses meta as string-to-string and drops the whole event on
    # anything else, so this is the wire contract, not a style preference.
    assert Enum.all?(meta, fn {key, value} -> is_binary(key) and is_binary(value) end)
  end

  test "a 1:1 chat titled after the sender does not repeat them" do
    Announce.announce(@source, %{@item | context: "Fixture Sender"})

    assert %{"content" => "Signal - Fixture Sender: hello there"} = await_channel()
  end

  test "a message missing sender, platform and title is still announced" do
    Announce.announce(@source, %{@item | sender: nil, platform: nil, context: nil})

    assert %{"content" => content, "meta" => meta} = await_channel()
    assert content == "chat - unknown sender: hello there"
    assert meta["sender"] == "unknown sender"
    assert meta["platform"] == "chat"
  end

  test "a multi-line message stays one line" do
    Announce.announce(@source, %{@item | preview: "first line\nsecond line"})

    assert %{"content" => content} = await_channel()
    assert content =~ "first line second line"
    refute content =~ "\n"
  end

  test "overflow is said out loud rather than swallowed" do
    Announce.announce_overflow(@source, 20)

    assert %{"content" => content, "meta" => meta} = await_channel()
    assert content =~ "more new messages"
    assert content =~ "20"
    assert meta["overflow"] == "true"
  end

  describe "preview/2" do
    test "collapses newlines, tabs and whitespace runs into one line" do
      assert Announce.preview("one\n\ntwo   three\tfour", 80) == "one two three four"
    end

    test "truncates past the cap and marks the cut" do
      assert Announce.preview(String.duplicate("a", 200), 10) ==
               String.duplicate("a", 10) <> "..."
    end

    test "text exactly at the cap is not marked" do
      assert Announce.preview(String.duplicate("a", 10), 10) == String.duplicate("a", 10)
    end

    test "nil and whitespace-only text render as a marker, never an empty line" do
      assert Announce.preview(nil, 80) == "(no text)"
      assert Announce.preview("   \n ", 80) == "(no text)"
    end
  end
end
