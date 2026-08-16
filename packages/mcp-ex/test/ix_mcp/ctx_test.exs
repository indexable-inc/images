defmodule IxMcp.CtxTest do
  use ExUnit.Case, async: true

  alias IxMcp.Blake3
  alias IxMcp.Ctx

  # 2000 numbered lines with a needle every 500th, so line numbers, byte
  # offsets and match positions are all independently checkable.
  defp corpus do
    Enum.map_join(1..2000, "", fn i ->
      needle = if rem(i, 500) == 0, do: " NEEDLE", else: ""
      "line #{i} " <> String.duplicate("x", 40) <> needle <> "\n"
    end)
  end

  test "a handle's id is blake3 of the attached content" do
    bytes = corpus()
    handle = Ctx.binary(bytes)

    assert handle.id == Blake3.hash_hex(bytes)
    assert handle.len == byte_size(bytes)
    assert handle.lines == 2000
  end

  test "the same bytes from a file and from memory get the same id" do
    bytes = corpus()
    path = Path.join(System.tmp_dir!(), "ctx-#{System.unique_integer([:positive])}.txt")
    File.write!(path, bytes)
    on_exit(fn -> File.rm(path) end)

    assert Ctx.file!(path).id == Ctx.binary(bytes).id
  end

  test "attach/1 reports a missing file instead of raising" do
    assert {:error, {:file, "/nonexistent/ctx", :enoent}} =
             Ctx.attach({:file, "/nonexistent/ctx"})
  end

  test "stat/1 is metadata only" do
    handle = Ctx.binary(corpus())
    stat = Ctx.stat(handle)

    assert stat.lines == 2000
    assert stat.whole?
    assert stat.offset == 0
    refute Map.has_key?(stat, :bytes_content)
    # Nothing in a stat is the content itself.
    refute stat |> inspect() |> String.contains?("xxxxxxxx")
  end

  describe "render discipline" do
    test "inspect shows metadata and a short prefix, never the content" do
      handle = Ctx.binary(corpus())
      rendered = inspect(handle)

      assert rendered =~ ~r/^#Ctx<[0-9a-f]{8} 98\.\d KB 2000 lines /
      # An interior line proves the body is absent, not merely elided.
      refute rendered =~ "line 1000"
      refute rendered =~ "NEEDLE"
      assert byte_size(rendered) < 120
    end

    test "a slice renders its window" do
      handle = corpus() |> Ctx.binary() |> Ctx.slice(lines: 3..5)
      assert inspect(handle) =~ ~r/^#Ctx<[0-9a-f]{8}@96\+143 143 B 3 lines /
    end

    test "an empty handle renders without special-casing" do
      assert inspect(Ctx.binary("")) == ~s(#Ctx<af1349b9 0 B 0 lines "">)
    end

    test "a struct field cannot leak the content through the default inspect" do
      # The impl is what enforces this; assert the protocol dispatches to it.
      assert Inspect.impl_for(Ctx.binary("x")) == Inspect.IxMcp.Ctx
    end
  end

  describe "slice/2" do
    test "byte ranges are clamped, not raised" do
      handle = Ctx.binary("0123456789")

      assert Ctx.read(Ctx.slice(handle, 2..4)) == "234"
      assert Ctx.read(Ctx.slice(handle, 0..10_000)) == "0123456789"
      assert Ctx.read(Ctx.slice(handle, 8..99)) == "89"
    end

    test "line ranges are 1-based and inclusive" do
      handle = Ctx.binary("a\nbb\nccc\ndddd\n")

      assert Ctx.read(Ctx.slice(handle, lines: 2..3)) == "bb\nccc"
      assert Ctx.read(Ctx.slice(handle, lines: 1..1)) == "a"
      assert Ctx.read(Ctx.slice(handle, lines: 99..100)) == ""
    end

    test "a slice of a slice composes against the same content id" do
      whole = Ctx.binary(corpus())
      outer = Ctx.slice(whole, lines: 3..5)
      inner = Ctx.slice(outer, 0..10)

      assert inner.id == whole.id
      assert inner.offset == outer.offset
      assert Ctx.read(inner) == "line 3 xxxx"

      # And the second hop is relative to the first, not to the whole: line 3
      # is 47 bytes, so byte 48 of THIS window is where line 4 starts.
      second = Ctx.slice(outer, 48..52)
      assert Ctx.read(second) == "line "
      assert second.offset == outer.offset + 48
      refute second.offset == 48
    end

    test "content_id/1 is the id the window's own bytes would get" do
      slice = corpus() |> Ctx.binary() |> Ctx.slice(lines: 3..5)

      assert Ctx.content_id(slice) == Blake3.hash_hex(Ctx.read(slice))
      refute Ctx.content_id(slice) == slice.id
    end

    test "key/1 identifies the window, and whole handles over equal bytes agree" do
      one = Ctx.binary("hello\n")
      two = Ctx.binary("hello\n")

      assert Ctx.key(one) == Ctx.key(two)
      assert Ctx.key(one) == "#{one.id}:0+6"
    end
  end

  describe "grep/2" do
    test "returns line numbers, the lines, and handles whose offsets are right" do
      handle = Ctx.binary(corpus())
      hits = Ctx.grep(handle, "NEEDLE")

      assert Enum.map(hits, fn {no, _line, _h} -> no end) == [500, 1000, 1500, 2000]

      for {no, line, slice} <- hits do
        assert line =~ "line #{no} "
        assert line =~ "NEEDLE"
        # The handle must name exactly the bytes of the reported line.
        assert Ctx.read(slice) == line
        assert slice.lines == 1
      end
    end

    test "accepts a regex" do
      handle = Ctx.binary("alpha\nbeta\ngamma\n")
      assert [{2, "beta", _}] = Ctx.grep(handle, ~r/^b/)
    end

    test "no match is an empty list" do
      assert Ctx.grep(Ctx.binary("alpha\n"), "omega") == []
    end
  end

  describe "chunks/2" do
    test "a count yields exactly that many line-aligned chunks that cover the whole" do
      bytes = corpus()
      chunks = bytes |> Ctx.binary() |> Ctx.chunks(4)

      assert Enum.map(chunks, & &1.lines) == [500, 500, 500, 500]
      assert Enum.all?(chunks, &String.starts_with?(Ctx.read(&1, limit: :all), "line "))

      # Coverage, exactly: the chunks are the file, minus the newlines the
      # split consumed at each boundary.
      assert Enum.map_join(chunks, "\n", &Ctx.read(&1, limit: :all)) <> "\n" == bytes
    end

    test "a byte target yields chunks under that size, still on line boundaries" do
      chunks = corpus() |> Ctx.binary() |> Ctx.chunks(bytes: 40_000)

      assert [_, _, _] = chunks
      assert Enum.all?(chunks, &(&1.len <= 40_000))
      assert Enum.all?(chunks, &String.starts_with?(Ctx.read(&1, limit: :all), "line "))
    end

    test "empty content has no chunks" do
      assert Ctx.chunks(Ctx.binary(""), 4) == []
      assert Ctx.chunks(Ctx.binary(""), bytes: 100) == []
    end

    test "fewer lines than chunks asked for yields one chunk per line" do
      assert [_, _] = Ctx.chunks(Ctx.binary("a\nb\n"), 8)
    end
  end

  describe "read/2" do
    test "caps by default and names the option that lifts the cap" do
      handle = Ctx.binary(corpus())
      read = Ctx.read(handle)

      assert byte_size(read) < handle.len
      assert read =~ "truncated at #{Ctx.read_cap()} of #{handle.len} bytes"
      assert read =~ "limit: :all"
    end

    test "limit: :all returns everything" do
      bytes = corpus()
      assert Ctx.read(Ctx.binary(bytes), limit: :all) == bytes
    end

    test "content under the cap comes back whole, with no marker" do
      assert Ctx.read(Ctx.binary("short\n")) == "short\n"
    end
  end

  test "term/1 attaches a binding by inspecting it with no limits" do
    handle = Ctx.term(Enum.to_list(1..500))

    assert handle.len > 1_000
    assert Ctx.read(handle, limit: :all) =~ "500]"
    assert match?({:term, _label}, handle.source)
  end
end
