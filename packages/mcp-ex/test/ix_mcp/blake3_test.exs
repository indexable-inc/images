defmodule IxMcp.Blake3Test do
  use ExUnit.Case, async: true

  alias IxMcp.Blake3

  # The official BLAKE3 test-vector input: byte i is i mod 251. The lengths are
  # the tree boundaries -- one chunk (1024), the first parent (1025), two
  # chunks, a non-power-of-two split (31337) -- because those are where a
  # merkle implementation goes wrong, and a suite of short strings would pass
  # while every one of them was broken.
  #
  # Oracle: b3sum 1.8.5 (nix), run over the same inputs on 2026-08-12. If one of
  # these ever fails, the implementation drifted; the vectors did not.
  @vectors [
    {0, "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"},
    {1, "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"},
    {2, "7b7015bb92cf0b318037702a6cdd81dee41224f734684c2c122cd6359cb1ee63"},
    {3, "e1be4d7a8ab5560aa4199eea339849ba8e293d55ca0a81006726d184519e647f"},
    {63, "e9bc37a594daad83be9470df7f7b3798297c3d834ce80ba85d6e207627b7db7b"},
    {64, "4eed7141ea4a5cd4b788606bd23f46e212af9cacebacdc7d1f4c6dc7f2511b98"},
    {65, "de1e5fa0be70df6d2be8fffd0e99ceaa8eb6e8c93a63f2d8d1c30ecb6b263dee"},
    {127, "d81293fda863f008c09e92fc382a81f5a0b4a1251cba1634016a0f86a6bd640d"},
    {128, "f17e570564b26578c33bb7f44643f539624b05df1a76c81f30acd548c44b45ef"},
    {129, "683aaae9f3c5ba37eaaf072aed0f9e30bac0865137bae68b1fde4ca2aebdcb12"},
    {1023, "10108970eeda3eb932baac1428c7a2163b0e924c9a9e25b35bba72b28f70bd11"},
    {1024, "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7"},
    {1025, "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444"},
    {2047, "58830fbf51a4423c573b164471690570e544cfe793bead46225664796b4b1467"},
    {2048, "e776b6028c7cd22a4d0ba182a8bf62205d2ef576467e838ed6f2529b85fba24a"},
    {2049, "5f4d72f40d7a5f82b15ca2b2e44b1de3c2ef86c426c95c1af0b6879522563030"},
    {3072, "b98cb0ff3623be03326b373de6b9095218513e64f1ee2edd2525c7ad1e5cffd2"},
    {3073, "7124b49501012f81cc7f11ca069ec9226cecb8a2c850cfe644e327d22d3e1cd3"},
    {4096, "015094013f57a5277b59d8475c0501042c0b642e531b0a1c8f58d2163229e969"},
    {4097, "9b4052b38f1c5fc8b1f9ff7ac7b27cd242487b3d890d15c96a1c25b8aa0fb995"},
    {5000, "ee78d92070de3df1c57c37002abf0a6b1a6589acdeef4d8ffac7cf3d9e8f2836"},
    {8192, "aae792484c8efe4f19e2ca7d371d8c467ffb10748d8a5a1ae579948f718a2a63"},
    {16_384, "f875d6646de28985646f34ee13be9a576fd515f76b5b0a26bb324735041ddde4"},
    {31_337, "ad35e0fa586b59f6c259aca598c9396dd42735f13edde2518ee2253631ae895a"}
  ]

  defp pattern(0), do: <<>>
  defp pattern(n), do: for(i <- 0..(n - 1), into: <<>>, do: <<rem(i, 251)>>)

  test "matches the reference digest at every tree boundary" do
    for {len, expected} <- @vectors do
      assert Blake3.hash_hex(pattern(len)) == expected, "length #{len} disagrees"
    end
  end

  test "the empty input has the published digest" do
    assert Blake3.hash_hex("") ==
             "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
  end

  # THE COMPATIBILITY PIN. IxMcp.Ctx ids must be the same address the
  # jj/forge object store uses, because a follow-up attaches context straight
  # from a jj tree and a memoized answer has to transfer between the two.
  #
  # Verified against the source rather than assumed: ix_hash::Content::compute
  # is `blake3::hash(data)` over the raw bytes (crates/ix/hash/src/lib.rs), and
  # jj's backend calls it on the file contents with nothing added --
  # `let local_id = ix_hash::Content::compute(&buf)` in
  # crates/jj/client/backend/src/backend.rs write_file. No domain prefix, no
  # length framing, no ObjectKind in the hash: the kind rides as its own wire
  # field. So there is nothing to mirror beyond bare blake3 over the content,
  # and this pair is the pin that catches it if that ever stops being true.
  test "a pinned (content, id) pair matches ix_hash::Content / jj FileId" do
    content = "the quick brown fox jumps over the lazy dog\n"

    assert Blake3.hash_hex(content) ==
             "2eacf908997728ba564ffe34c4b8e55d22c3c0fb58b58403305f0af80a9cd419"
  end

  test "hash/1 returns 32 raw bytes and hash_hex/1 its lowercase hex" do
    digest = Blake3.hash("ix")
    assert byte_size(digest) == 32
    assert Base.encode16(digest, case: :lower) == Blake3.hash_hex("ix")
  end

  test "hash_file/1 agrees with hash/1 on the same bytes" do
    path = Path.join(System.tmp_dir!(), "blake3-#{System.unique_integer([:positive])}.bin")
    bytes = pattern(5000)
    File.write!(path, bytes)
    on_exit(fn -> File.rm(path) end)

    assert Blake3.hash_file(path) == {:ok, Blake3.hash(bytes)}
    assert {:ok, digest} = Blake3.hash_file(path)
    assert Base.encode16(digest, case: :lower) == Blake3.hash_hex(bytes)
  end
end
