defmodule IxMcp.Read do
  @moduledoc """
  File reading for cells, aliased in the workspace prelude as `Read`. The
  old `read` MCP tool also evaluated workspace expressions, but inside a
  cell the language itself does that (#3532), so only the file path
  survived the fold-in: `Read.file/1` returns a whole file, `Read.file/2,3`
  slice a 1-based inclusive line range.
  """

  @doc "Read `path`; `first`/`last` select a 1-based inclusive line range."
  @spec file(Path.t(), pos_integer() | nil, pos_integer() | nil) :: String.t()
  def file(path, first \\ nil, last \\ nil)

  def file(path, nil, nil), do: File.read!(path)

  def file(path, first, last) do
    lines = path |> File.read!() |> String.split("\n")
    first = max(first || 1, 1)
    last = min(last || length(lines), length(lines))
    lines |> Enum.slice((first - 1)..(last - 1)//1) |> Enum.join("\n")
  end
end
