defmodule IxMcp.WebTest do
  use ExUnit.Case, async: false

  alias IxMcp.Web

  # Every test here is offline on purpose. The transport is `:httpc` against a
  # third-party API, so a test that reached the network would measure exa's
  # uptime rather than this module. What IS ours is the request we build, the
  # shape we hand back to a cell, and what happens when the key is missing --
  # so those are what the seams below are pure for.

  setup do
    saved = System.get_env("EXA_API_KEY")

    on_exit(fn ->
      if saved, do: System.put_env("EXA_API_KEY", saved), else: System.delete_env("EXA_API_KEY")
    end)

    :ok
  end

  describe "request bodies" do
    test "search defaults to text-bearing results" do
      body = Web.search_body("erlang httpc", [])

      assert body["query"] == "erlang httpc"
      assert body["numResults"] == 8
      assert body["contents"] == %{"text" => true}
    end

    test "results: and text: are the two knobs that change cost" do
      body = Web.search_body("q", results: 3, text: false)

      assert body["numResults"] == 3
      assert body["contents"] == %{"text" => false}
    end

    test "contents takes the urls verbatim" do
      assert Web.contents_body(["https://a.example", "https://b.example"]) ==
               %{"urls" => ["https://a.example", "https://b.example"], "text" => true}
    end
  end

  describe "response shaping" do
    @hit %{
      "url" => "https://example.com/x",
      "title" => "X",
      "author" => "A",
      "publishedDate" => "2026-01-01T00:00:00.000Z",
      "text" => "hello"
    }

    test "a hit becomes an atom-keyed map a cell can pattern match" do
      assert [result] = Web.parse_results(%{"results" => [@hit]}, [])

      assert result == %{
               url: "https://example.com/x",
               title: "X",
               author: "A",
               published: "2026-01-01T00:00:00.000Z",
               text: "hello"
             }
    end

    test "missing optional fields come back nil rather than absent" do
      assert [result] = Web.parse_results(%{"results" => [%{"url" => "https://u"}]}, [])
      assert result.url == "https://u"
      assert result.title == nil
      assert result.text == nil
    end

    test "an unexpected shape raises instead of returning an empty list" do
      # A silent [] here would read downstream as "the web has nothing on
      # that", which is the wrong thing to tell an agent about an API change.
      assert_raise RuntimeError, ~r/unexpected response shape/, fn ->
        Web.parse_results(%{"error" => "nope"}, [])
      end
    end
  end

  describe "clipping" do
    test "text under the cap is untouched" do
      assert Web.clip("short", 100) == "short"
    end

    test "a clipped document says it was clipped and names the way out" do
      clipped = Web.clip(String.duplicate("a", 50), 10)

      assert String.starts_with?(clipped, String.duplicate("a", 10))
      assert clipped =~ "truncated at 10 chars"
      assert clipped =~ "chars: :all"
    end

    test "chars: :all lifts the cap" do
      long = String.duplicate("a", 50)
      assert Web.clip(long, :all) == long
    end

    test "nil text stays nil" do
      assert Web.clip(nil, 10) == nil
    end
  end

  describe "credentials" do
    test "a missing key raises and names the variable" do
      System.delete_env("EXA_API_KEY")

      assert_raise RuntimeError, ~r/EXA_API_KEY/, fn -> Web.api_key() end
    end

    test "a present key is returned as-is" do
      System.put_env("EXA_API_KEY", "test-key")
      assert Web.api_key() == "test-key"
    end
  end

  describe "prelude" do
    test "Web is aliased in every cell" do
      # The module is only useful if a cell can say `Web.` without an alias, and
      # nothing else in this suite would notice if it fell out of @prelude.
      IxMcp.Workspace.reset()

      {summary, _out} = IxMcp.Jobs.run(~s|Web.clip("abc", :all)|, budget: 5, intent: "prelude")

      assert summary.status == :done
      assert summary.result == ~s|"abc"|
    end
  end
end
