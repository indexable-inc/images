defmodule IxMcp.Inbox.MailTest do
  use ExUnit.Case, async: false

  alias IxMcp.Inbox.Mail

  # Invented throughout, and the addresses use the reserved `.invalid` TLD so
  # no fixture can name a real mailbox. This tree is public.
  @hits [
    %{
      id: "fixture-g2",
      thread_id: "fixture-t2",
      from: "Fixture Two <two@example.invalid>",
      subject: "Second fixture subject",
      snippet: "the newer body"
    },
    %{
      id: "fixture-g1",
      thread_id: "fixture-t1",
      from: "Fixture One <one@example.invalid>",
      subject: "First fixture subject",
      snippet: "the older body"
    }
  ]

  @since ~U[2026-01-02 03:04:05Z]

  defmodule Ready do
    @moduledoc false
    def status, do: %{configured: true, signed_in: true, missing_scopes: []}
    def search(query, opts), do: send(self(), {:searched, query, opts}) && {:ok, hits()}
    defp hits, do: Process.get(:hits, [])
  end

  defmodule SignedOut do
    @moduledoc false
    def status, do: %{configured: true, signed_in: false, missing_scopes: []}
    def search(_query, _opts), do: {:ok, []}
  end

  defmodule Unscoped do
    @moduledoc false
    def status, do: %{configured: true, signed_in: true, missing_scopes: ["readonly"]}
    def search(_query, _opts), do: {:ok, []}
  end

  defmodule Broken do
    @moduledoc false
    def status, do: %{configured: true, signed_in: true, missing_scopes: []}
    def search(_query, _opts), do: {:error, %{message: "fixture quota exceeded"}}
  end

  defp ready_state(opts \\ []) do
    {:ok, state} =
      Mail.init(Keyword.merge([mail: Ready, query: "in:inbox -from:me"], opts))

    state
  end

  describe "init/1" do
    test "runs when the grant is present and scoped" do
      assert {:ok, %{query: "in:inbox -from:me"}} =
               Mail.init(mail: Ready, query: "in:inbox -from:me")
    end

    test "is absent, silently, when the kernel was never signed in" do
      assert :ignore = Mail.init(mail: SignedOut)
    end

    test "is absent when the grant is missing a read scope" do
      assert :ignore = Mail.init(mail: Unscoped)
    end

    test "the off switch wins over a working grant" do
      System.put_env("IX_MCP_MAIL_WATCH", "0")
      on_exit(fn -> System.delete_env("IX_MCP_MAIL_WATCH") end)

      assert :ignore = Mail.init(mail: Ready)
    end

    test "the default query watches inbox arrivals rather than unread state" do
      System.delete_env("IX_MCP_MAIL_WATCH_QUERY")

      assert {:ok, %{query: query}} = Mail.init(mail: Ready)
      # Arrival is the event: a mail already read on a phone still matters to
      # a session that has not heard of it.
      assert query == "in:inbox -from:me"
      refute query =~ "is:unread"
    end
  end

  describe "fetch/3" do
    test "the watermark rides Gmail's own after: term, in epoch seconds" do
      Process.put(:hits, [])

      assert {:ok, [], false, _state} = Mail.fetch(ready_state(), @since, 20)

      assert_receive {:searched, query, opts}
      assert query == "in:inbox -from:me after:#{DateTime.to_unix(@since)}"
      assert opts[:limit] == 20
    end

    test "hits are announced oldest first, the reverse of what the API returns" do
      Process.put(:hits, @hits)

      assert {:ok, items, _more?, _state} = Mail.fetch(ready_state(), @since, 20)

      assert Enum.map(items, & &1.id) == ["fixture-g1", "fixture-g2"]
      assert [oldest | _rest] = items
      assert oldest.platform == "email"
      assert oldest.sender == "Fixture One <one@example.invalid>"
      assert oldest.context == "First fixture subject"
      assert oldest.preview == "the older body"
    end

    test "a full page is reported as overflow" do
      Process.put(:hits, @hits)

      # The client returns the newest `limit` hits and says nothing about a
      # remainder, so "exactly full" and "more than full" are one observation.
      assert {:ok, _items, true, _state} = Mail.fetch(ready_state(), @since, 2)
      assert {:ok, _items, false, _state2} = Mail.fetch(ready_state(), @since, 3)
    end

    test "a client error becomes a short detail the watcher can back off on" do
      assert {:error, detail} = Mail.fetch(%{mail: Broken, query: "in:inbox"}, @since, 20)
      assert detail == "fixture quota exceeded"
    end
  end
end
