defmodule IxMcp.Blake3 do
  @moduledoc """
  BLAKE3-256 over raw bytes, in pure Elixir.

  ## Why this exists

  `IxMcp.Ctx` handles are named by the same content id the ix object store
  uses, so the SAME bytes attached from a local file and (a follow-up) from
  a jj tree produce the SAME id, and a memoized `IxMcp.LM` answer transfers
  across both. That id is `ix_hash::Content` (`crates/ix/hash/src/lib.rs`),
  which is `blake3::hash(data)` over the RAW bytes, lowercase hex — no
  domain prefix, no length framing, no keyed or derive-key mode. jj's
  `FileId` is exactly that (`crates/jj/client/backend/src/backend.rs`,
  `write_file` computes `ix_hash::Content::compute(&buf)` over the file
  bytes and the server checks the id matches). `ObjectKind::File` travels
  as a separate wire field and never enters the hash. So there is nothing
  to mirror beyond plain BLAKE3, and `test/blake3_test.exs` pins one
  (content, id) pair as a KAT so a future drift on either side is caught.

  Never sha256: this repo's content addressing is blake3 everywhere we
  control it.

  ## Cost

  Pure Elixir, ~1.5 MB/s on aarch64-darwin, so hashing is the dominant
  cost of `Ctx.attach/1` on large inputs. OTP's `:crypto` has blake2 but
  not blake3 (checked on OTP 28), and a Rustler dependency would drag
  `mix.lock` plus a `pins.json` FOD refresh and cargo-inside-a-FOD into
  this package. The follow-up is a `IxMcp.NifApp`-loaded binding to the
  in-tree `ix-hash` crate, which is why `hash/1` is the only entry point
  the rest of the package calls: swapping the implementation is a
  one-module change.
  """

  import Bitwise

  @mask 0xFFFFFFFF
  @chunk_len 1024
  @block_len 64

  @flag_chunk_start 1
  @flag_chunk_end 2
  @flag_parent 4
  @flag_root 8

  @iv0 0x6A09E667
  @iv1 0xBB67AE85
  @iv2 0x3C6EF372
  @iv3 0xA54FF53A
  @iv4 0x510E527F
  @iv5 0x9B05688C
  @iv6 0x1F83D9AB
  @iv7 0x5BE0CD19

  @typedoc "A 32-byte BLAKE3 digest."
  @type digest :: binary()

  @doc """
  The 32-byte BLAKE3 digest of `data`.

  Matches `blake3::hash` / `ix_hash::Content::compute`, so the hex form is
  an ix object-store id.
  """
  @spec hash(binary()) :: digest()
  def hash(data) when is_binary(data), do: words_to_binary(root_words(data))

  @doc """
  `hash/1` as lowercase hex — the form ix ids are written in.
  """
  @spec hash_hex(binary()) :: String.t()
  def hash_hex(data) when is_binary(data), do: data |> hash() |> Base.encode16(case: :lower)

  @doc """
  Hash a file's contents.

  Reads the whole file: this is a hashing function, not a streaming one,
  and every caller in this package already holds the bytes.
  """
  @spec hash_file(Path.t()) :: {:ok, digest()} | {:error, File.posix()}
  def hash_file(path) do
    with {:ok, bytes} <- File.read(path), do: {:ok, hash(bytes)}
  end

  # ── tree ──────────────────────────────────────────────────────────────
  #
  # One chunk is 1024 bytes. Anything longer splits into a left subtree of
  # the largest power-of-two chunk count STRICTLY LESS than the total, and
  # a right subtree with the rest; the top node carries ROOT.

  @spec root_words(binary()) :: tuple()
  defp root_words(data) when byte_size(data) <= @chunk_len,
    do: chunk_words(data, 0, @flag_root)

  defp root_words(data) do
    {left, right} = split(data)
    left_cv = subtree_cv(left, 0)
    right_cv = subtree_cv(right, div(byte_size(left), @chunk_len))
    parent_words(left_cv, right_cv, @flag_root)
  end

  @spec subtree_cv(binary(), non_neg_integer()) :: tuple()
  defp subtree_cv(data, counter) when byte_size(data) <= @chunk_len,
    do: chunk_words(data, counter, 0)

  defp subtree_cv(data, counter) do
    {left, right} = split(data)
    left_cv = subtree_cv(left, counter)
    right_cv = subtree_cv(right, counter + div(byte_size(left), @chunk_len))
    parent_words(left_cv, right_cv, 0)
  end

  @spec split(binary()) :: {binary(), binary()}
  defp split(data) do
    chunks = div(byte_size(data) + @chunk_len - 1, @chunk_len)
    left_bytes = half_chunks(1, chunks) * @chunk_len
    <<left::binary-size(^left_bytes), right::binary>> = data
    {left, right}
  end

  @spec half_chunks(pos_integer(), pos_integer()) :: pos_integer()
  defp half_chunks(p, chunks) when p * 2 < chunks, do: half_chunks(p * 2, chunks)
  defp half_chunks(p, _chunks), do: p

  @spec parent_words(tuple(), tuple(), non_neg_integer()) :: tuple()
  defp parent_words(left_cv, right_cv, extra) do
    {l0, l1, l2, l3, l4, l5, l6, l7} = left_cv
    {r0, r1, r2, r3, r4, r5, r6, r7} = right_cv
    block = {l0, l1, l2, l3, l4, l5, l6, l7, r0, r1, r2, r3, r4, r5, r6, r7}
    compress(iv(), block, 0, @block_len, bor(@flag_parent, extra))
  end

  @spec chunk_words(binary(), non_neg_integer(), non_neg_integer()) :: tuple()
  defp chunk_words(data, counter, extra),
    do: do_chunk(data, iv(), counter, @flag_chunk_start, extra)

  @spec do_chunk(binary(), tuple(), non_neg_integer(), non_neg_integer(), non_neg_integer()) ::
          tuple()
  defp do_chunk(block, cv, counter, start_flag, extra) when byte_size(block) <= @block_len do
    flags = bor(bor(start_flag, @flag_chunk_end), extra)
    compress(cv, block_words(block), counter, byte_size(block), flags)
  end

  defp do_chunk(data, cv, counter, start_flag, extra) do
    <<block::binary-size(@block_len), rest::binary>> = data
    cv = compress(cv, block_words(block), counter, @block_len, start_flag)
    do_chunk(rest, cv, counter, 0, extra)
  end

  # ── compression ───────────────────────────────────────────────────────

  @spec compress(tuple(), tuple(), non_neg_integer(), non_neg_integer(), non_neg_integer()) ::
          tuple()
  defp compress(cv, m0, counter, block_len, flags) do
    {c0, c1, c2, c3, c4, c5, c6, c7} = cv

    s0 =
      {c0, c1, c2, c3, c4, c5, c6, c7, @iv0, @iv1, @iv2, @iv3, band(counter, @mask),
       band(bsr(counter, 32), @mask), block_len, flags}

    m1 = permute(m0)
    m2 = permute(m1)
    m3 = permute(m2)
    m4 = permute(m3)
    m5 = permute(m4)
    m6 = permute(m5)

    s =
      s0
      |> round_fn(m0)
      |> round_fn(m1)
      |> round_fn(m2)
      |> round_fn(m3)
      |> round_fn(m4)
      |> round_fn(m5)
      |> round_fn(m6)

    {t0, t1, t2, t3, t4, t5, t6, t7, t8, t9, t10, t11, t12, t13, t14, t15} = s

    {bxor(t0, t8), bxor(t1, t9), bxor(t2, t10), bxor(t3, t11), bxor(t4, t12), bxor(t5, t13),
     bxor(t6, t14), bxor(t7, t15)}
  end

  @spec round_fn(tuple(), tuple()) :: tuple()
  defp round_fn(state, m) do
    {s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11, s12, s13, s14, s15} = state
    {m0, m1, m2, m3, m4, m5, m6, m7, m8, m9, m10, m11, m12, m13, m14, m15} = m

    {s0, s4, s8, s12} = g(s0, s4, s8, s12, m0, m1)
    {s1, s5, s9, s13} = g(s1, s5, s9, s13, m2, m3)
    {s2, s6, s10, s14} = g(s2, s6, s10, s14, m4, m5)
    {s3, s7, s11, s15} = g(s3, s7, s11, s15, m6, m7)

    {s0, s5, s10, s15} = g(s0, s5, s10, s15, m8, m9)
    {s1, s6, s11, s12} = g(s1, s6, s11, s12, m10, m11)
    {s2, s7, s8, s13} = g(s2, s7, s8, s13, m12, m13)
    {s3, s4, s9, s14} = g(s3, s4, s9, s14, m14, m15)

    {s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11, s12, s13, s14, s15}
  end

  @spec g(
          non_neg_integer(),
          non_neg_integer(),
          non_neg_integer(),
          non_neg_integer(),
          non_neg_integer(),
          non_neg_integer()
        ) :: {non_neg_integer(), non_neg_integer(), non_neg_integer(), non_neg_integer()}
  defp g(a, b, c, d, mx, my) do
    a = band(a + b + mx, @mask)
    d = rotr(bxor(d, a), 16)
    c = band(c + d, @mask)
    b = rotr(bxor(b, c), 12)
    a = band(a + b + my, @mask)
    d = rotr(bxor(d, a), 8)
    c = band(c + d, @mask)
    b = rotr(bxor(b, c), 7)
    {a, b, c, d}
  end

  @spec permute(tuple()) :: tuple()
  defp permute({m0, m1, m2, m3, m4, m5, m6, m7, m8, m9, m10, m11, m12, m13, m14, m15}) do
    _ = m1
    {m2, m6, m3, m10, m7, m0, m4, m13, m1, m11, m12, m5, m9, m14, m15, m8}
  end

  @spec rotr(non_neg_integer(), pos_integer()) :: non_neg_integer()
  defp rotr(x, n), do: band(bor(bsr(x, n), bsl(x, 32 - n)), @mask)

  @spec iv() :: tuple()
  defp iv, do: {@iv0, @iv1, @iv2, @iv3, @iv4, @iv5, @iv6, @iv7}

  @spec block_words(binary()) :: tuple()
  defp block_words(block) do
    pad = @block_len - byte_size(block)

    <<w0::little-32, w1::little-32, w2::little-32, w3::little-32, w4::little-32, w5::little-32,
      w6::little-32, w7::little-32, w8::little-32, w9::little-32, w10::little-32, w11::little-32,
      w12::little-32, w13::little-32, w14::little-32, w15::little-32>> =
      block <> :binary.copy(<<0>>, pad)

    {w0, w1, w2, w3, w4, w5, w6, w7, w8, w9, w10, w11, w12, w13, w14, w15}
  end

  @spec words_to_binary(tuple()) :: binary()
  defp words_to_binary({w0, w1, w2, w3, w4, w5, w6, w7}) do
    <<w0::little-32, w1::little-32, w2::little-32, w3::little-32, w4::little-32, w5::little-32,
      w6::little-32, w7::little-32>>
  end
end
