defmodule SymphonyElixir.DSL.Parser.Lexer do
  @moduledoc """
  Tokenize `.sym` source into a flat token list, each token carrying its
  1-based line and column so the parser can attach a precise span to any
  diagnostic.

  The lexer owns three non-obvious decisions:

  - String literals are emitted as a list of `{:lit, text}` and `{:ref,
    path}` segments. `"summarize ${session.report}"` becomes `[{:lit,
    "summarize "}, {:ref, ["session", "report"]}]`, so the parser can
    lower interpolation to a `concat` of pure values without re-scanning.
  - A bare `${path}` outside a string is its own `:interp` token, the
    common shape for `when ${gate.ok} { ... }`.
  - `#` starts a line comment to end of line. Comments and whitespace
    advance the position counters but emit no token.
  """

  @keywords ~w(workflow agent exec subrun when every of map as skill inline timeout true false null)

  @type segment :: {:lit, String.t()} | {:ref, [String.t()]}

  @type token :: %{
          type: atom(),
          value: term(),
          line: pos_integer(),
          column: pos_integer()
        }

  @spec tokenize(String.t()) :: {:ok, [token()]} | {:error, map()}
  def tokenize(source) when is_binary(source) do
    do_tokenize(source, 1, 1, [])
  end

  defp do_tokenize("", _line, _col, acc), do: {:ok, Enum.reverse(acc)}

  defp do_tokenize(<<"\n", rest::binary>>, line, _col, acc),
    do: do_tokenize(rest, line + 1, 1, acc)

  defp do_tokenize(<<c::utf8, rest::binary>>, line, col, acc) when c in [?\s, ?\t, ?\r],
    do: do_tokenize(rest, line, col + 1, acc)

  defp do_tokenize(<<"#", rest::binary>>, line, col, acc) do
    {_comment, rest_after, advanced} = take_line(rest, col + 1)
    do_tokenize(rest_after, line, advanced, acc)
  end

  defp do_tokenize(<<"<-", rest::binary>>, line, col, acc),
    do: do_tokenize(rest, line, col + 2, [tok(:larrow, "<-", line, col) | acc])

  defp do_tokenize(<<"{", rest::binary>>, line, col, acc),
    do: do_tokenize(rest, line, col + 1, [tok(:lbrace, "{", line, col) | acc])

  defp do_tokenize(<<"}", rest::binary>>, line, col, acc),
    do: do_tokenize(rest, line, col + 1, [tok(:rbrace, "}", line, col) | acc])

  defp do_tokenize(<<"[", rest::binary>>, line, col, acc),
    do: do_tokenize(rest, line, col + 1, [tok(:lbracket, "[", line, col) | acc])

  defp do_tokenize(<<"]", rest::binary>>, line, col, acc),
    do: do_tokenize(rest, line, col + 1, [tok(:rbracket, "]", line, col) | acc])

  defp do_tokenize(<<":", rest::binary>>, line, col, acc),
    do: do_tokenize(rest, line, col + 1, [tok(:colon, ":", line, col) | acc])

  defp do_tokenize(<<",", rest::binary>>, line, col, acc),
    do: do_tokenize(rest, line, col + 1, [tok(:comma, ",", line, col) | acc])

  defp do_tokenize(<<"=", rest::binary>>, line, col, acc),
    do: do_tokenize(rest, line, col + 1, [tok(:equals, "=", line, col) | acc])

  defp do_tokenize(<<"\"", rest::binary>>, line, col, acc) do
    case scan_string(rest, line, col + 1, [], []) do
      {:ok, segments, rest_after, new_line, new_col} ->
        do_tokenize(rest_after, new_line, new_col, [tok(:string, segments, line, col) | acc])

      {:error, _} = err ->
        err
    end
  end

  defp do_tokenize(<<"${", rest::binary>>, line, col, acc) do
    case scan_interp(rest, line, col + 2) do
      {:ok, path, rest_after, new_col} ->
        do_tokenize(rest_after, line, new_col, [tok(:interp, path, line, col) | acc])

      {:error, _} = err ->
        err
    end
  end

  defp do_tokenize(<<c::utf8, _::binary>> = bin, line, col, acc) when c in ?0..?9 do
    {number, rest, advanced} = scan_number(bin, col)
    do_tokenize(rest, line, advanced, [number_token(number, line, col) | acc])
  end

  defp do_tokenize(<<c::utf8, _::binary>> = bin, line, col, acc)
       when c in ?a..?z or c in ?A..?Z or c == ?_ do
    {word, rest, advanced} = scan_ident(bin, col)
    type = if word in @keywords, do: :keyword, else: :ident
    do_tokenize(rest, line, advanced, [tok(type, word, line, col) | acc])
  end

  defp do_tokenize(<<c::utf8, _::binary>>, line, col, _acc) do
    {:error, %{message: "unexpected character #{inspect(<<c::utf8>>)}", line: line, column: col, got: <<c::utf8>>}}
  end

  # --- scanners -----------------------------------------------------------

  defp take_line(bin, col), do: take_line(bin, col, [])
  defp take_line(<<"\n", _::binary>> = rest, col, acc), do: {IO.iodata_to_binary(Enum.reverse(acc)), rest, col}
  defp take_line("", col, acc), do: {IO.iodata_to_binary(Enum.reverse(acc)), "", col}
  defp take_line(<<c::utf8, rest::binary>>, col, acc), do: take_line(rest, col + 1, [<<c::utf8>> | acc])

  # Scan a double-quoted string into interpolation segments. `lit` is the
  # literal-character accumulator for the current run; `segs` is the list
  # of finished segments. A `${` opens an interpolation that closes at `}`.
  defp scan_string(<<"\"", rest::binary>>, line, col, lit, segs) do
    segs = flush_lit(lit, segs)
    {:ok, Enum.reverse(segs), rest, line, col + 1}
  end

  defp scan_string(<<"\\", c::utf8, rest::binary>>, line, col, lit, segs) do
    scan_string(rest, line, col + 2, [unescape(c) | lit], segs)
  end

  defp scan_string(<<"${", rest::binary>>, line, col, lit, segs) do
    segs = flush_lit(lit, segs)

    case scan_interp(rest, line, col + 2) do
      {:ok, path, rest_after, new_col} ->
        scan_string(rest_after, line, new_col, [], [{:ref, path} | segs])

      {:error, _} = err ->
        err
    end
  end

  defp scan_string(<<"\n", _::binary>>, line, col, _lit, _segs) do
    {:error, %{message: "unterminated string", line: line, column: col, got: :newline}}
  end

  defp scan_string("", line, col, _lit, _segs) do
    {:error, %{message: "unterminated string", line: line, column: col, got: :eof}}
  end

  defp scan_string(<<c::utf8, rest::binary>>, line, col, lit, segs) do
    scan_string(rest, line, col + 1, [<<c::utf8>> | lit], segs)
  end

  defp flush_lit([], segs), do: segs
  defp flush_lit(lit, segs), do: [{:lit, IO.iodata_to_binary(Enum.reverse(lit))} | segs]

  defp unescape(?n), do: "\n"
  defp unescape(?t), do: "\t"
  defp unescape(?r), do: "\r"
  defp unescape(?"), do: "\""
  defp unescape(?\\), do: "\\"
  defp unescape(c), do: <<c::utf8>>

  # An interpolation reference is a dotted path of identifiers terminated
  # by `}`. `${session.report.ok}` -> ["session", "report", "ok"].
  defp scan_interp(bin, line, col), do: scan_interp(bin, line, col, [], [])

  defp scan_interp(<<"}", rest::binary>>, line, col, current, acc) do
    path = Enum.reverse(flush_path(current, acc))

    case path do
      [] -> {:error, %{message: "empty interpolation", line: line, column: col, got: :empty_interp}}
      _ -> {:ok, path, rest, col + 1}
    end
  end

  defp scan_interp(<<".", rest::binary>>, line, col, current, acc) do
    scan_interp(rest, line, col + 1, [], flush_path(current, acc))
  end

  defp scan_interp(<<c::utf8, rest::binary>>, line, col, current, acc)
       when c in ?a..?z or c in ?A..?Z or c in ?0..?9 or c == ?_ do
    scan_interp(rest, line, col + 1, [<<c::utf8>> | current], acc)
  end

  defp scan_interp("", line, col, _current, _acc) do
    {:error, %{message: "unterminated interpolation", line: line, column: col, got: :eof}}
  end

  defp scan_interp(<<c::utf8, _::binary>>, line, col, _current, _acc) do
    {:error, %{message: "invalid character in interpolation #{inspect(<<c::utf8>>)}", line: line, column: col, got: <<c::utf8>>}}
  end

  defp flush_path([], acc), do: acc
  defp flush_path(current, acc), do: [IO.iodata_to_binary(Enum.reverse(current)) | acc]

  defp scan_number(bin, col), do: scan_number(bin, col, [], false)

  defp scan_number(<<c::utf8, rest::binary>>, col, acc, dot?) when c in ?0..?9,
    do: scan_number(rest, col + 1, [<<c::utf8>> | acc], dot?)

  defp scan_number(<<".", c::utf8, rest::binary>>, col, acc, false) when c in ?0..?9,
    do: scan_number(rest, col + 2, [<<c::utf8>>, "." | acc], true)

  defp scan_number(rest, col, acc, dot?) do
    text = IO.iodata_to_binary(Enum.reverse(acc))
    {%{text: text, float?: dot?}, rest, col}
  end

  defp number_token(%{text: text, float?: true}, line, col),
    do: tok(:float, String.to_float(text), line, col)

  defp number_token(%{text: text, float?: false}, line, col),
    do: tok(:int, String.to_integer(text), line, col)

  defp scan_ident(bin, col), do: scan_ident(bin, col, [])

  defp scan_ident(<<c::utf8, rest::binary>>, col, acc)
       when c in ?a..?z or c in ?A..?Z or c in ?0..?9 or c == ?_,
       do: scan_ident(rest, col + 1, [<<c::utf8>> | acc])

  defp scan_ident(rest, col, acc), do: {IO.iodata_to_binary(Enum.reverse(acc)), rest, col}

  defp tok(type, value, line, col), do: %{type: type, value: value, line: line, column: col}
end
