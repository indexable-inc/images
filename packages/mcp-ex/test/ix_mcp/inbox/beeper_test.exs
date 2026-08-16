defmodule IxMcp.Inbox.BeeperTest do
  use ExUnit.Case, async: false

  alias IxMcp.Inbox.Beeper

  @moduletag :tmp_dir

  # Invented throughout: this tree is public, so no fixture may carry a real
  # contact, handle, chat title, chat id, or token. The shape is copied from a
  # live response; the contents are not.
  @payload %{
    "items" => [
      %{
        "id" => "fixture-m2",
        "chatID" => "fixture-chat-1",
        "senderName" => "Fixture One",
        "timestamp" => "2026-01-02T00:00:02.000Z",
        "type" => "text",
        "text" => "second"
      },
      %{
        "id" => "fixture-m1",
        "chatID" => "fixture-chat-1",
        "senderName" => "Fixture One",
        "timestamp" => "2026-01-02T00:00:01.000Z",
        "type" => "text",
        "text" => "first"
      },
      %{
        "id" => "fixture-m0",
        "chatID" => "fixture-chat-2",
        "senderName" => "Fixture Two",
        "timestamp" => "2026-01-02T00:00:00.000Z",
        "type" => "image"
      },
      %{
        "id" => "fixture-mine",
        "chatID" => "fixture-chat-1",
        "senderName" => "Fixture Self",
        "timestamp" => "2026-01-02T00:00:03.000Z",
        "isSender" => true,
        "text" => "my own message"
      }
    ],
    "chats" => %{
      "fixture-chat-1" => %{"network" => "Signal", "title" => "Fixture Group"},
      "fixture-chat-2" => %{"network" => "WhatsApp", "title" => "Fixture Two"}
    },
    "hasMore" => true
  }

  @since ~U[2026-01-02 00:00:00Z]

  # Records the URL each fetch built, so the query is assertable without a
  # network: the flags are the whole behaviour of this source.
  defp canned(payload) do
    test = self()

    fn url, _token ->
      send(test, {:requested, url})
      {:ok, JSON.encode!(payload)}
    end
  end

  defp state(opts) do
    {:ok, state} =
      Beeper.init(Keyword.merge([token: "fixture-token", http: canned(@payload)], opts))

    state
  end

  describe "init/1" do
    test "is absent, silently, when the token file does not exist", %{tmp_dir: dir} do
      missing = Path.join(dir, "no-such-token")
      System.put_env("IX_MCP_BEEPER_TOKEN_FILE", missing)
      on_exit(fn -> System.delete_env("IX_MCP_BEEPER_TOKEN_FILE") end)

      assert :ignore = Beeper.init([])
    end

    test "is absent when the token file is empty", %{tmp_dir: dir} do
      empty = Path.join(dir, "empty-token")
      File.write!(empty, "   \n")
      System.put_env("IX_MCP_BEEPER_TOKEN_FILE", empty)
      on_exit(fn -> System.delete_env("IX_MCP_BEEPER_TOKEN_FILE") end)

      assert :ignore = Beeper.init([])
    end

    test "reads a token file, trimming the trailing newline", %{tmp_dir: dir} do
      file = Path.join(dir, "token")
      File.write!(file, "fixture-token\n")
      System.put_env("IX_MCP_BEEPER_TOKEN_FILE", file)
      on_exit(fn -> System.delete_env("IX_MCP_BEEPER_TOKEN_FILE") end)

      assert {:ok, %{token: "fixture-token"}} = Beeper.init([])
    end

    test "the off switch wins over a present token" do
      System.put_env("IX_MCP_BEEPER_WATCH", "0")
      on_exit(fn -> System.delete_env("IX_MCP_BEEPER_WATCH") end)

      assert :ignore = Beeper.init(token: "fixture-token")
    end
  end

  describe "fetch/3" do
    test "normalizes a page into items, oldest first, with the chat side-load" do
      assert {:ok, items, more?, _state} = Beeper.fetch(state([]), @since, 20)

      assert Enum.map(items, & &1.id) == ["fixture-m0", "fixture-m1", "fixture-m2"]
      assert more? == true

      # The network and title come from `chats`, keyed by the message's chatID;
      # without that side-load a line could only name an account id.
      assert [oldest | _rest] = items
      assert oldest.platform == "WhatsApp"
      assert oldest.context == "Fixture Two"
      # An attachment-only message names its kind rather than rendering blank.
      assert oldest.preview == "[image]"

      assert Enum.find(items, &(&1.id == "fixture-m1")).platform == "Signal"
    end

    test "the user's own messages never announce" do
      {:ok, items, _more?, _state} = Beeper.fetch(state([]), @since, 20)

      refute Enum.any?(items, &(&1.id == "fixture-mine"))
    end

    test "the watermark and the inbound-only filter ride the query" do
      {:ok, _items, _more?, _state} = Beeper.fetch(state([]), @since, 7)

      assert_receive {:requested, url}
      assert url =~ "dateAfter=2026-01-02T00%3A00%3A00Z"
      assert url =~ "sender=others"
      assert url =~ "limit=7"
      # Loud by default: everything is announced, including chats the user
      # marked Muted or Low Priority.
      assert url =~ "excludeLowPriority=false"
      assert url =~ "includeMuted=true"
    end

    test "the quiet knob respects the user's own Muted and Low Priority marks" do
      {:ok, _items, _more?, _state} = Beeper.fetch(state(quiet?: true), @since, 20)

      assert_receive {:requested, url}
      assert url =~ "excludeLowPriority=true"
      assert url =~ "includeMuted=false"
    end

    test "a page with no items is not an error" do
      empty = %{"items" => [], "chats" => %{}, "hasMore" => false}

      assert {:ok, [], false, _state} = Beeper.fetch(state(http: canned(empty)), @since, 20)
    end

    test "a missing items key is tolerated rather than crashing the feed" do
      assert {:ok, [], false, _state} = Beeper.fetch(state(http: canned(%{})), @since, 20)
    end

    test "a transport failure comes back as an error the watcher can back off on" do
      failing = fn _url, _token -> {:error, "Beeper Desktop unreachable: :econnrefused"} end

      assert {:error, detail} = Beeper.fetch(state(http: failing), @since, 20)
      assert detail =~ "unreachable"
    end

    test "an undecodable body is reported without quoting the body" do
      # The body can carry message text, and a sweep failure is logged: a log
      # outlives the session, so content must not reach the error string.
      leaky = fn _url, _token -> {:ok, "not json: fixture-private-content"} end

      assert {:error, detail} = Beeper.fetch(state(http: leaky), @since, 20)
      refute detail =~ "fixture-private-content"
    end
  end
end
