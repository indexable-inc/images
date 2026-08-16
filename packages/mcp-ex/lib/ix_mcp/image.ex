defmodule IxMcp.Image do
  @moduledoc """
  Images as first-class cell values, aliased in the workspace prelude as
  `Image`. A cell whose result is (or contains) `Image.read(path)` puts a
  real MCP image content block on the exec reply -- base64 data plus mime
  type, rendered by the client as a picture -- instead of a text dump of
  bytes:

      Image.read("/tmp/screenshot.png")            # the reply shows the image
      %{before: Image.read(a), after: Image.read(b)}  # both ride the reply

  The struct carries raw bytes; base64 happens once, at reply serialization
  (`to_content/1`). `Inspect` renders a short placeholder so an image bound
  in the workspace never floods a text reply or a checkpoint diff.

  Only the reply of the exec call that produced the value carries the
  blocks: a backgrounded job's images do not survive into the durable
  ledger (which stores rendered text), so re-read the path in a quick
  foreground cell when a background job's images are wanted.
  """

  @enforce_keys [:mime, :data]
  defstruct [:mime, :data, :path]

  @type t :: %__MODULE__{mime: String.t(), data: binary(), path: Path.t() | nil}

  # Raw-byte ceiling per image. Base64 inflates 4/3, and multi-megabyte
  # blocks help no client; the error names the fix rather than truncating a
  # picture into garbage.
  @max_bytes 6 * 1024 * 1024

  # How many images one reply will carry, and how deep collect/1 walks.
  @max_images 8
  @max_depth 6

  @doc """
  Read an image file into an `%IxMcp.Image{}`. The format is sniffed from
  the bytes (PNG, JPEG, GIF, WebP), never trusted from the extension.
  Raises with the supported list on anything else, and on files over
  #{div(@max_bytes, 1024 * 1024)} MiB (downscale first, e.g.
  `sips -Z 1200 file.png` on macOS).
  """
  @spec read(Path.t()) :: t()
  def read(path) do
    data = File.read!(path)

    case sniff(data) do
      {:ok, mime} ->
        check_size!(data, path)
        %__MODULE__{mime: mime, data: data, path: Path.expand(path)}

      :error ->
        raise ArgumentError,
              "#{path} is not a supported image (PNG, JPEG, GIF, WebP); " <>
                "it starts with #{inspect(binary_part(data, 0, min(byte_size(data), 8)))}. " <>
                "Convert it first (macOS: sips -s format png in.heic --out out.png)."
    end
  end

  @doc """
  Wrap already-in-memory bytes (a generated chart, a fetched image) as an
  `%IxMcp.Image{}`. The mime type is sniffed unless given explicitly --
  pass `mime:` for formats without a magic number.
  """
  @spec from_binary(binary(), keyword()) :: t()
  def from_binary(data, opts \\ []) when is_binary(data) do
    mime =
      case {Keyword.get(opts, :mime), sniff(data)} do
        {given, _sniffed} when is_binary(given) ->
          given

        {nil, {:ok, sniffed}} ->
          sniffed

        {nil, :error} ->
          raise ArgumentError,
                "could not sniff an image format from these bytes; " <>
                  "pass mime: explicitly (e.g. mime: \"image/png\")"
      end

    check_size!(data, "binary")
    %__MODULE__{mime: mime, data: data, path: nil}
  end

  @doc """
  Every image inside `value`, walking lists, maps, tuples and structs a few
  levels deep -- how the exec reply finds the images its cell returned.
  Bounded (#{@max_images} images, depth #{@max_depth}) so a pathological
  value cannot stall a reply.
  """
  @spec collect(term()) :: [t()]
  def collect(value) do
    {images, _count} = collect(value, @max_depth, {[], 0})
    Enum.reverse(images)
  end

  # Short-circuits at @max_images so a pathological cell value (a
  # million-element list, say) costs one bounded walk, not an unbounded one,
  # on the reply path of every finished exec.
  defp collect(_value, _depth, {_images, count} = acc) when count >= @max_images, do: acc
  defp collect(_value, 0, acc), do: acc
  defp collect(%__MODULE__{} = image, _depth, {images, count}), do: {[image | images], count + 1}

  defp collect(list, depth, acc) when is_list(list) do
    Enum.reduce_while(list, acc, fn item, acc ->
      case collect(item, depth - 1, acc) do
        {_images, count} = acc when count >= @max_images -> {:halt, acc}
        acc -> {:cont, acc}
      end
    end)
  end

  defp collect(tuple, depth, acc) when is_tuple(tuple) do
    collect(Tuple.to_list(tuple), depth, acc)
  end

  defp collect(%_struct{} = other_struct, depth, acc) do
    collect(Map.values(Map.from_struct(other_struct)), depth, acc)
  end

  defp collect(map, depth, acc) when is_map(map) do
    collect(Map.values(map), depth, acc)
  end

  defp collect(_value, _depth, acc), do: acc

  @doc "The MCP image content block for this image (base64 data + mime type)."
  @spec to_content(t()) :: map()
  def to_content(%__MODULE__{} = image) do
    %{"type" => "image", "data" => Base.encode64(image.data), "mimeType" => image.mime}
  end

  defp check_size!(data, what) do
    if byte_size(data) > @max_bytes do
      raise ArgumentError,
            "#{what} is #{byte_size(data)} bytes; images over #{@max_bytes} " <>
              "(#{div(@max_bytes, 1024 * 1024)} MiB) do not ride a reply usefully. " <>
              "Downscale first (macOS: sips -Z 1200 <file>)."
    end

    :ok
  end

  defp sniff(<<0x89, "PNG", 0x0D, 0x0A, 0x1A, 0x0A, _rest::binary>>), do: {:ok, "image/png"}
  defp sniff(<<0xFF, 0xD8, 0xFF, _rest::binary>>), do: {:ok, "image/jpeg"}
  defp sniff(<<"GIF87a", _rest::binary>>), do: {:ok, "image/gif"}
  defp sniff(<<"GIF89a", _rest::binary>>), do: {:ok, "image/gif"}
  defp sniff(<<"RIFF", _size::binary-size(4), "WEBP", _rest::binary>>), do: {:ok, "image/webp"}
  defp sniff(_data), do: :error
end

defimpl Inspect, for: IxMcp.Image do
  # The struct holds megabytes of raw bytes; rendering it must not. The
  # placeholder names what the value is and where it came from, which is all
  # a text surface (workspace warnings, Jobs.history, checkpoints) needs.
  def inspect(image, _opts) do
    source = if image.path, do: " #{image.path}", else: ""
    "#IxMcp.Image<#{image.mime}, #{byte_size(image.data)} bytes#{source}>"
  end
end
