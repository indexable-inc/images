#!/usr/bin/env bash

# `clone-store-paths` deduplicates with APFS copy-on-write clones instead of
# hard links. Two properties have to hold together, and they are the whole
# point of the setting: the files must share storage (the saving), and each
# must keep its own inode (so dyld's F_GETPATH cannot report a sibling store
# path, which is what made hardlink dedup unusable on macOS).

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
    skipTest "APFS clones are macOS-only"
fi

clearStoreIfPossible

store="$TEST_ROOT/store-cloned"

mkdir -p "$TEST_ROOT/a" "$TEST_ROOT/b"
# Big enough to occupy real extents, and identical in both paths.
# NB: not `yes ... | head -c`; that leaves `yes` killed by SIGPIPE, which the
# harness's `pipefail` reports as a test failure.
head -c 300000 /dev/zero | tr '\0' 'D' > "$TEST_ROOT/a/dup"
cp "$TEST_ROOT/a/dup" "$TEST_ROOT/b/dup"

pathA=$(nix --store "$store" store add-path "$TEST_ROOT/a" --name a)
pathB=$(nix --store "$store" store add-path "$TEST_ROOT/b" --name b)

hashBefore=$(nix hash path "$store/nix/store/$(basename "$pathA")")

nix --store "$store" --option clone-store-paths true store optimise

realA="$store/nix/store/$(basename "$pathA")/dup"
realB="$store/nix/store/$(basename "$pathB")/dup"

# Contents survive deduplication.
[[ "$(nix hash path "$store/nix/store/$(basename "$pathA")")" == "$hashBefore" ]]
cmp "$realA" "$realB"

# Distinct inodes: this is what a hard link would NOT give, and what keeps
# dyld from resolving one store path's file to another store path's name.
inoA=$(statOf %i %i "$realA")
inoB=$(statOf %i %i "$realB")
[[ "$inoA" != "$inoB" ]]

# ...and the link count stays 1, i.e. nothing was hard-linked behind our back.
[[ "$(statOf %h %l "$realA")" == 1 ]]

# Re-running must be a no-op rather than re-cloning: the optimiser recognises
# files that already share their storage.
nix --store "$store" --option clone-store-paths true store optimise
[[ "$(nix hash path "$store/nix/store/$(basename "$pathA")")" == "$hashBefore" ]]
[[ "$(statOf %i %i "$realA")" == "$inoA" ]]

nix --store "$store" store verify --all

echo "DARWIN-CLONE-OK"
