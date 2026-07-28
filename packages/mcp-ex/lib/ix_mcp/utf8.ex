defmodule IxMcp.UTF8 do
  @moduledoc """
  Lossy-but-loud UTF-8 sanitation for text that rides the JSON-RPC wire.

  #3523 made stdio a transparent byte pipe so valid UTF-8 crosses the wire
  untouched; this module is the outbound half of that contract (#3538). The
  OTP JSON encoder raises on invalid UTF-8, and `:unicode.characters_to_binary/1`
  answers invalid input with an `{:error, ...}` tuple instead of a binary --
  so one cell printing a compiled binary could poison its job record and
  kill the whole connection. Here valid UTF-8 passes through byte-identical
  (the #3523 transparency, extended to output) and every byte that cannot
  ride becomes a visible `\\xNN` escape: loud enough to see, harmless to
  encode.
  """

  @doc """
  Convert chardata to a valid UTF-8 binary, escaping whatever cannot be
  converted: invalid bytes as `\\xNN`, invalid codepoints as `\\u{...}`.
  """
  @spec sanitize(IO.chardata()) :: String.t()
  def sanitize(chardata) do
    case :unicode.characters_to_binary(chardata) do
      binary when is_binary(binary) -> binary
      {_error_or_incomplete, converted, rest} -> sanitize_rest(rest, [converted])
    end
  end

  # Iterate with an iodata accumulator instead of concatenating around a
  # recursive call: the #3538 payload was a multi-megabyte compiled binary,
  # where per-escape stack growth would trade one crash for another.
  # `:error` stops at the first invalid unit, `:incomplete` at a truncated
  # trailing multibyte sequence; either way the offending unit is escaped
  # and conversion resumes right after it.
  defp sanitize_rest(rest, acc) do
    {escaped, tail} = escape_head(rest)

    case :unicode.characters_to_binary(tail) do
      binary when is_binary(binary) ->
        IO.iodata_to_binary(Enum.reverse([binary, escaped | acc]))

      {_error_or_incomplete, converted, next_rest} ->
        sanitize_rest(next_rest, [converted, escaped | acc])
    end
  end

  # `rest` keeps the shape of the input -- a binary tail, or the unconsumed
  # elements of a chardata list -- with the offending unit at its head.
  defp escape_head(<<byte, tail::binary>>), do: {escape_byte(byte), tail}
  defp escape_head([head | tail]) when is_integer(head), do: {escape_codepoint(head), tail}
  defp escape_head([]), do: {"", []}

  defp escape_head([head | tail]) do
    {escaped, inner_tail} = escape_head(head)
    {escaped, [inner_tail | tail]}
  end

  defp escape_byte(byte), do: "\\x" <> Base.encode16(<<byte>>)

  defp escape_codepoint(codepoint), do: "\\u{" <> Integer.to_string(codepoint, 16) <> "}"

  @doc """
  The longest prefix of `binary` at most `max_bytes` long that does not end
  inside a multibyte codepoint. A byte cap that split a sequence would
  reintroduce the exact invalid-UTF-8 payload this module exists to keep
  off the wire (#3538).
  """
  @spec truncate(String.t(), non_neg_integer()) :: String.t()
  def truncate(binary, max_bytes) when byte_size(binary) <= max_bytes, do: binary
  def truncate(binary, max_bytes), do: cut(binary, max_bytes)

  defp cut(_binary, 0), do: ""

  # A continuation byte (0b10xxxxxx) right after the cut means the cut
  # falls mid-sequence; back up until the next byte starts a codepoint.
  defp cut(binary, cut_at) do
    # `^cut_at`: from Elixir 1.20 a variable used in a bitstring `size(...)`
    # must be pinned when it comes from outside the match, since an unpinned
    # name there reads as a new binding rather than the existing value.
    case binary do
      <<_::binary-size(^cut_at), 0b10::2, _::bitstring>> -> cut(binary, cut_at - 1)
      <<keep::binary-size(^cut_at), _::binary>> -> keep
    end
  end
end
