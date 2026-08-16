#!/usr/bin/env bash

# APFS transparent (decmpfs) store compression is invisible above the
# filesystem: a path added with `compress-store-paths` enabled must hash
# identically to the same path added without it, and must read back
# byte-for-byte. The compression is an on-disk representation change only, so
# if this test can ever fail the setting is unsafe at any speed.

source common.sh

# The test harness puts GNU coreutils on PATH, while a bare macOS shell has the
# BSD tools, and /usr/bin is outside the build sandbox (so hardcoding
# /usr/bin/stat fails with EPERM). Ask whichever stat is present.
statOf() { # statOf <gnu-format> <bsd-format> <file>
    stat -c "$1" "$3" 2>/dev/null || stat -f "$2" "$3"
}

# BSD `ls -O` would show the `compressed` flag, but GNU ls has no such option
# and no stat format exposes BSD file flags. Compression is observable without
# either: a decmpfs file's data fork is truncated, so its allocated blocks fall
# far below its apparent size.
isCompressed() {
    local size blocks
    size=$(statOf %s %z "$1")
    blocks=$(statOf %b %b "$1")
    [[ $((blocks * 512)) -lt $((size / 2)) ]]
}

if [[ "$(uname -s)" != Darwin ]]; then
    skipTest "APFS compression is macOS-only"
fi

clearStoreIfPossible

mkdir -p "$TEST_ROOT/src/sub"
# Highly compressible so the block-saving threshold is comfortably met, plus an
# incompressible file to exercise the stored-chunk marker path, plus a file
# larger than one 64 KiB chunk to exercise the resource-fork offset table.
# NB: not `yes ... | head -c`, which leaves `yes` killed by SIGPIPE and so
# fails under the harness's `pipefail`. `head` reading a device directly exits
# on its own and `tr` then sees a clean EOF.
head -c 400000 /dev/zero | tr '\0' 'A' > "$TEST_ROOT/src/compressible"
head -c 65536 /dev/urandom > "$TEST_ROOT/src/incompressible"
printf 'tiny' > "$TEST_ROOT/src/sub/tiny"
ln -s compressible "$TEST_ROOT/src/link"

expected=$(nix hash path "$TEST_ROOT/src")

pathPlain=$(nix store add-path --option compress-store-paths false "$TEST_ROOT/src" --name src)
pathCompressed=$(nix --store "$TEST_ROOT/store-compressed" store add-path \
    --option compress-store-paths true "$TEST_ROOT/src" --name src)

# The store path is derived from the NAR hash, so equal names prove the
# compression did not perturb the hash.
[[ "$(basename "$pathPlain")" == "$(basename "$pathCompressed")" ]]

realCompressed="$TEST_ROOT/store-compressed/nix/store/$(basename "$pathCompressed")"

# The contents must read back identically...
[[ "$(nix hash path "$realCompressed")" == "$expected" ]]
diff -r "$TEST_ROOT/src" "$realCompressed"

# ...and the store must consider the path valid on its own terms.
nix --store "$TEST_ROOT/store-compressed" store verify --all

# Something must actually have been compressed, or this test proves nothing.
# The big repetitive file must have been compressed...
isCompressed "$realCompressed/compressible"
# ...and the random one must not: compressing it would cost CPU on every read
# and free no blocks.
! isCompressed "$realCompressed/incompressible"

# The canonical metadata must survive compression: writing the decmpfs xattrs
# bumps the mtime and needs write permission, and the store asserts on both
# when it canonicalises a path again.
for f in "$realCompressed"/compressible "$realCompressed"/incompressible; do
    [[ "$(statOf %Y %m "$f")" == 1 ]]
    [[ "$(statOf %a %Lp "$f")" == 444 ]]
done

# Canonicalising again must not destroy a compressed path: clearing
# UF_COMPRESSED truncates the data fork, so the flag has to survive.
nix --store "$TEST_ROOT/store-compressed" store repair --all 2>/dev/null || true
[[ "$(nix hash path "$realCompressed")" == "$expected" ]]

echo "DARWIN-COMPRESS-OK"
