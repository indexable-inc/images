defmodule IxMcp.UTF8Test do
  use ExUnit.Case, async: true

  alias IxMcp.UTF8

  describe "sanitize/1" do
    test "valid UTF-8 passes through byte-identical, multibyte included" do
      # The #3523 transparency contract, extended to output: what can ride
      # as-is must ride as-is.
      assert UTF8.sanitize("snow ☃") == "snow ☃"
      assert UTF8.sanitize([?a, "bc", [~c"de"]]) == "abcde"
    end

    test "invalid bytes become visible \\xNN escapes" do
      assert UTF8.sanitize(<<0xFF, "abc">>) == "\\xFFabc"
      assert UTF8.sanitize(<<"ok", 0xC0, 0x80, "end">>) == "ok\\xC0\\x80end"
    end

    test "the IO.puts shape: chardata list with an embedded invalid binary" do
      # IO.puts hands the group leader [data, ?\n]; #3538's repro rode in
      # exactly like this.
      assert UTF8.sanitize([<<0xFF, "a">>, ?\n]) == "\\xFFa\n"
    end

    test "a truncated trailing multibyte sequence is escaped, not dropped" do
      assert UTF8.sanitize(<<"a", 0xE2, 0x98>>) == "a\\xE2\\x98"
    end

    test "invalid codepoints escape as \\u{...}" do
      assert UTF8.sanitize([0x110000]) == "\\u{110000}"
    end
  end

  describe "truncate/2" do
    test "input at or under the cap is untouched" do
      assert UTF8.truncate("abc", 3) == "abc"
      assert UTF8.truncate("abc", 10) == "abc"
    end

    test "never cuts inside a multibyte sequence" do
      # "☃" is three bytes: budgets landing mid-snowman back up to "a".
      assert UTF8.truncate("a☃b", 4) == "a☃"
      assert UTF8.truncate("a☃b", 3) == "a"
      assert UTF8.truncate("a☃b", 2) == "a"
      assert UTF8.truncate("☃", 1) == ""
    end
  end

  describe "truncate_tail/2" do
    test "keeps the END of the text, which is where a diagnostic lives" do
      assert UTF8.truncate_tail("abcdef", 3) == "def"
      assert UTF8.truncate_tail("abc", 10) == "abc"
      assert UTF8.truncate_tail("", 4) == ""
    end

    test "never begins inside a multibyte codepoint" do
      # A byte-exact tail of an arrow run starts mid-sequence; the result must
      # still be valid UTF-8, or it raises {:invalid_byte, _} from JSON.encode!
      # on the reply path (#3538).
      arrows = String.duplicate("→", 50)

      for max <- 1..12 do
        tail = UTF8.truncate_tail(arrows, max)
        assert String.valid?(tail), "invalid tail at max=#{max}"
        assert byte_size(tail) <= max
      end
    end

    test "is the mirror of truncate/2, not the same function" do
      assert UTF8.truncate("abcdef", 3) == "abc"
      assert UTF8.truncate_tail("abcdef", 3) == "def"
    end
  end
end
