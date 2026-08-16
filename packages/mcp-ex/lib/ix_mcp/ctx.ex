defmodule IxMcp.Ctx do
  @moduledoc """
  Context handles: big inputs live here as VARIABLES, not in the window.

  This is the first of the three RLM primitives (`IxMcp.Ctx`, `IxMcp.LM`,
  `IxMcp.EventLog`); `IxMcp.RLM` has the design note and the paper
  citation.

  ## Render discipline (the whole point)

  A handle NEVER renders its contents. Inspect shows the content id, the
  byte length, the line count and a short prefix, so a cell can return a
  handle for a 40 MB log and cost the caller one line:

      iex> h = Ctx.file!("/var/log/big.log")
      #Ctx<9f2ab1c4 41.2 MB 812_004 lines "2026-08-12T00:00:01Z start...">

  Bytes enter the model's window only through `read/2`, which is
  hard-capped and says so in its own truncation marker. Everything else
  (`stat/1`, `grep/2`, `chunks/2`, `slice/2`) is metadata or more handles.
  That is what makes an unbounded input tractable: the model navigates it
  programmatically and only ever materializes the parts it argued for.

  ## Ids are ix content ids

  `id` is `IxMcp.Blake3.hash_hex/1` of the ATTACHED content, which is
  exactly `ix_hash::Content` / jj's `FileId` over the same bytes (see
  `IxMcp.Blake3` for what was mirrored and why). So the same bytes
  attached from a local path and — once the jj source lands — from a jj
  tree yield the same id, and a memoized `IxMcp.LM` answer over one
  transfers to the other.

  A handle is a WINDOW on that content: `id` names the whole, `offset` and
  `len` name the window. `slice/2` and `grep/2` return windows sharing the
  parent's id and binary (sub-binaries, no copy), so slice-of-slice
  composes and `id` stays the id of the thing that was attached. When you
  want the id of a window's own bytes, `content_id/1` computes it.

  ## Sources

  `attach/1` takes a tagged source so that a path and a blob of content
  can never be confused for each other:

      Ctx.attach({:file, "/tmp/x.json"})   # or Ctx.file!("/tmp/x.json")
      Ctx.attach({:binary, bytes})         # or Ctx.binary(bytes)
      Ctx.attach({:term, some_binding})    # or Ctx.term(binding)

  `{:jj, rev, path}` is the named follow-up: the `source` field is a
  tagged tuple precisely so it slots in without touching handle identity.
  Job outputs and CI runs land the same way.
  """

  alias IxMcp.Blake3

  @read_cap 4_000
  @prefix_len 48

  defstruct [:id, :bytes, :offset, :len, :lines, :source]

  @typedoc """
  A window on attached content.

  * `id` — blake3 hex of the whole attached content (an ix object id)
  * `bytes` — the whole content; windows share it, so slices copy nothing
  * `offset`/`len` — this window, in bytes, against the whole
  * `lines` — line count of this window
  * `source` — tagged provenance, e.g. `{:file, path}`
  """
  @type t :: %__MODULE__{
          id: String.t(),
          bytes: binary(),
          offset: non_neg_integer(),
          len: non_neg_integer(),
          lines: non_neg_integer(),
          source: source()
        }

  @type source :: {:file, Path.t()} | {:binary, :inline} | {:term, String.t()}

  @doc """
  Attach a source and get a handle. The content is hashed once, here.
  """
  @spec attach(
          {:file, Path.t()}
          | {:binary, binary()}
          | {:term, term()}
        ) :: {:ok, t()} | {:error, term()}
  def attach({:file, path}) do
    case File.read(path) do
      {:ok, bytes} -> {:ok, wrap(bytes, {:file, path})}
      {:error, reason} -> {:error, {:file, path, reason}}
    end
  end

  def attach({:binary, bytes}) when is_binary(bytes), do: {:ok, wrap(bytes, {:binary, :inline})}

  def attach({:term, term}) do
    bytes =
      if is_binary(term),
        do: term,
        else: inspect(term, limit: :infinity, printable_limit: :infinity)

    {:ok, wrap(bytes, {:term, label(term)})}
  end

  @doc "`attach/1` for a file, raising on a read error."
  @spec file!(Path.t()) :: t()
  def file!(path) do
    case attach({:file, path}) do
      {:ok, handle} ->
        handle

      {:error, {:file, ^path, reason}} ->
        raise File.Error, reason: reason, action: "read", path: path
    end
  end

  @doc "`attach/1` for raw bytes already in hand."
  @spec binary(binary()) :: t()
  def binary(bytes) when is_binary(bytes), do: wrap(bytes, {:binary, :inline})

  @doc """
  `attach/1` for an existing binding: binaries as-is, anything else via
  `inspect/2` with no limits, which is the REPL-shaped thing to do.
  """
  @spec term(term()) :: t()
  def term(value) do
    {:ok, handle} = attach({:term, value})
    handle
  end

  @doc """
  Metadata only: id, window, byte length, line count, source, and the
  content id of this window when it is not the whole.
  """
  @spec stat(t()) :: map()
  def stat(%__MODULE__{} = h) do
    %{
      id: h.id,
      bytes: h.len,
      lines: h.lines,
      offset: h.offset,
      whole?: whole?(h),
      source: h.source
    }
  end

  @doc """
  blake3 hex of THIS window's bytes.

  Equal to `id` for a whole handle; for a slice it is the id the same
  bytes would get if they were attached on their own. Costs a hash.
  """
  @spec content_id(t()) :: String.t()
  def content_id(%__MODULE__{} = h) do
    if whole?(h), do: h.id, else: Blake3.hash_hex(data(h))
  end

  @doc """
  The memoization identity of a window: `"<id>:<offset>+<len>"`.

  Whole handles over identical bytes agree on this string regardless of
  which source produced them, which is what makes `IxMcp.LM` cache hits
  transfer across sources and machines.
  """
  @spec key(t()) :: String.t()
  def key(%__MODULE__{} = h), do: "#{h.id}:#{h.offset}+#{h.len}"

  @doc """
  A sub-window, by bytes (`0..99`, `bytes: 0..99`) or lines
  (`lines: 10..20`, 1-based and inclusive).

  Returns a handle, never content. Ranges are clamped to the window
  rather than raising, so `slice(h, 0..10_000_000)` is the whole thing.
  Offsets compose: a slice of a slice is a window on the same content id.
  """
  @spec slice(t(), Range.t() | [{:bytes, Range.t()} | {:lines, Range.t()}]) :: t()
  def slice(%__MODULE__{} = h, first..last//_ = range) do
    _ = range
    byte_slice(h, first, last)
  end

  def slice(%__MODULE__{} = h, bytes: first..last//_), do: byte_slice(h, first, last)

  def slice(%__MODULE__{} = h, lines: first..last//_) when first >= 1 do
    index = line_index(h)
    lines = Enum.slice(index, (first - 1)..(last - 1))

    case lines do
      [] ->
        byte_slice(h, 0, -1)

      _ ->
        {start, _} = List.first(lines)
        {last_off, last_len} = List.last(lines)
        byte_slice(h, start, last_off + last_len - 1)
    end
  end

  @doc """
  Lines matching `pattern` (a `Regex` or a plain substring) as
  `{line_no, line, handle}`, where `handle` is that line's window.

  `line` is the matched line itself, so a grep of a 40 MB log costs the
  window only the lines that matched — and the handles let a sub-call be
  aimed at exactly those bytes.
  """
  @spec grep(t(), Regex.t() | binary()) :: [{pos_integer(), String.t(), t()}]
  def grep(%__MODULE__{} = h, pattern) do
    bytes = data(h)

    h
    |> line_index()
    |> Enum.with_index(1)
    |> Enum.filter(fn {{off, len}, _no} -> line_match?(binary_part(bytes, off, len), pattern) end)
    |> Enum.map(fn {{off, len}, no} ->
      {no, binary_part(bytes, off, len), byte_slice(h, off, off + len - 1)}
    end)
  end

  @doc """
  Split into handles on LINE BOUNDARIES: `chunks(h, 8)` for eight
  roughly-equal chunks, `chunks(h, bytes: 40_000)` for chunks of about
  that many bytes.

  Line alignment is not a detail: it is what makes a chunk a thing a
  sub-model can reason about, and what makes `IxMcp.LM.map/3` over the
  chunks of a grown log re-pay only for the chunks that are new.
  """
  @spec chunks(t(), pos_integer() | [{:bytes, pos_integer()}]) :: [t()]
  def chunks(%__MODULE__{} = h, count) when is_integer(count) and count > 0 do
    index = line_index(h)
    per = max(div(length(index) + count - 1, count), 1)

    index
    |> Enum.chunk_every(per)
    |> Enum.map(fn lines ->
      {start, _} = List.first(lines)
      {last_off, last_len} = List.last(lines)
      byte_slice(h, start, last_off + last_len - 1)
    end)
  end

  def chunks(%__MODULE__{len: 0} = _h, bytes: _target), do: []

  def chunks(%__MODULE__{} = h, bytes: target) when is_integer(target) and target > 0 do
    h
    |> line_index()
    |> Enum.reduce([], fn {off, len}, acc ->
      case acc do
        [{start, run} | rest] when run + len + 1 <= target -> [{start, off + len - start} | rest]
        acc -> [{off, len} | acc]
      end
    end)
    |> Enum.reverse()
    |> Enum.map(fn {off, len} -> byte_slice(h, off, off + len - 1) end)
  end

  @doc """
  Materialize bytes into the caller's window. HARD CAPPED.

  This is the only function in this module that returns content, and the
  cap is the point: `limit: :all` exists but must be typed on purpose, and
  the truncation marker names it so a model that needs more knows how to
  ask.
  """
  @spec read(t(), keyword()) :: String.t()
  def read(%__MODULE__{} = h, opts \\ []) do
    limit = Keyword.get(opts, :limit, @read_cap)
    bytes = data(h)

    cond do
      limit == :all -> bytes
      byte_size(bytes) <= limit -> bytes
      true -> binary_part(bytes, 0, limit) <> marker(byte_size(bytes), limit)
    end
  end

  @doc """
  The default `read/2` cap, in bytes.
  """
  @spec read_cap() :: pos_integer()
  def read_cap, do: @read_cap

  @doc false
  @spec data(t()) :: binary()
  def data(%__MODULE__{} = h), do: binary_part(h.bytes, h.offset, h.len)

  @doc false
  @spec prefix(t()) :: String.t()
  def prefix(%__MODULE__{} = h) do
    h
    |> data()
    |> binary_part(0, min(h.len, @prefix_len))
    |> String.replace(["\n", "\r", "\t"], " ")
    |> String.slice(0, @prefix_len)
  end

  # ── internals ─────────────────────────────────────────────────────────

  @spec wrap(binary(), source()) :: t()
  defp wrap(bytes, source) do
    %__MODULE__{
      id: Blake3.hash_hex(bytes),
      bytes: bytes,
      offset: 0,
      len: byte_size(bytes),
      lines: count_lines(bytes),
      source: source
    }
  end

  @spec whole?(t()) :: boolean()
  defp whole?(%__MODULE__{} = h), do: h.offset == 0 and h.len == byte_size(h.bytes)

  @spec byte_slice(t(), integer(), integer()) :: t()
  defp byte_slice(%__MODULE__{} = h, first, last) do
    first = max(first, 0)
    last = min(last, h.len - 1)
    len = max(last - first + 1, 0)
    offset = h.offset + min(first, h.len)
    window = if len == 0, do: <<>>, else: binary_part(h.bytes, offset, len)

    %__MODULE__{h | offset: offset, len: len, lines: count_lines(window)}
  end

  @spec count_lines(binary()) :: non_neg_integer()
  defp count_lines(<<>>), do: 0

  defp count_lines(bytes) do
    newlines = length(:binary.matches(bytes, "\n"))
    if :binary.last(bytes) == ?\n, do: newlines, else: newlines + 1
  end

  # {offset, len} per line, relative to the window, newline excluded.
  @spec line_index(t()) :: [{non_neg_integer(), non_neg_integer()}]
  defp line_index(%__MODULE__{len: 0}), do: []

  defp line_index(%__MODULE__{} = h) do
    bytes = data(h)
    breaks = :binary.matches(bytes, "\n")

    {index, pos} =
      Enum.map_reduce(breaks, 0, fn {at, 1}, pos -> {{pos, at - pos}, at + 1} end)

    if pos < byte_size(bytes), do: index ++ [{pos, byte_size(bytes) - pos}], else: index
  end

  @spec line_match?(binary(), Regex.t() | binary()) :: boolean()
  defp line_match?(line, %Regex{} = pattern), do: Regex.match?(pattern, line)
  defp line_match?(line, pattern) when is_binary(pattern), do: String.contains?(line, pattern)

  @spec marker(non_neg_integer(), pos_integer()) :: String.t()
  defp marker(total, limit) do
    "\n[Ctx: truncated at #{limit} of #{total} bytes; pass limit: :all, " <>
      "or slice/2 and grep/2 to aim at the part you want]"
  end

  @spec label(term()) :: String.t()
  defp label(term) when is_binary(term), do: "binary"
  defp label(term), do: term |> inspect(limit: 3, printable_limit: 24) |> String.slice(0, 40)
end

defimpl Inspect, for: IxMcp.Ctx do
  alias IxMcp.Ctx

  @doc false
  @spec inspect(Ctx.t(), Inspect.Opts.t()) :: Inspect.Algebra.t()
  def inspect(%Ctx{} = h, _opts) do
    Inspect.Algebra.concat([
      "#Ctx<",
      String.slice(h.id, 0, 8),
      window(h),
      " ",
      size(h.len),
      " ",
      Integer.to_string(h.lines),
      " lines ",
      Kernel.inspect(Ctx.prefix(h)),
      ">"
    ])
  end

  @spec window(Ctx.t()) :: String.t()
  defp window(%Ctx{offset: 0} = h) do
    if h.len == byte_size(h.bytes), do: "", else: "@0+#{h.len}"
  end

  defp window(%Ctx{} = h), do: "@#{h.offset}+#{h.len}"

  @spec size(non_neg_integer()) :: String.t()
  defp size(n) when n < 1_024, do: "#{n} B"
  defp size(n) when n < 1_048_576, do: "#{Float.round(n / 1_024, 1)} KB"
  defp size(n), do: "#{Float.round(n / 1_048_576, 1)} MB"
end
