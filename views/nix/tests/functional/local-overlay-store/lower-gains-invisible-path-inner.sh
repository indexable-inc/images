#!/usr/bin/env bash

# The lower store is allowed to gain store objects while an overlay over it is
# mounted. The kernel does not promise to *show* them there: a lookup that
# missed before the lower store gained the entry leaves a negative dentry that
# nothing revalidates, so the merged directory keeps answering ENOENT for a
# directory that is sitting in the lower layer. Every other test in this
# directory hides that by calling `remountOverlayfs` right after touching the
# lower store; a caller that cannot remount -- and the new mount API refuses to
# reconfigure an overlay mount, which is why common.sh forces the old one --
# has no such option.
#
# What must not happen then is for the store to call such a path valid. It
# would copy the lower store's registration into its own database and every
# later reader would be sent to bytes it cannot reach, surfacing much later as
# `path '<store dir>/...' does not exist` from whatever first dumps or copies
# it. Report it invalid instead: true of this store, and self-healing, because
# the caller then puts the object in the upper layer where it is readable.
#
# This is the no-remount sibling of lower-gains-output.sh, and it needs no race
# to set up: the negative dentry is seeded with a plain `stat` before the lower
# store gains the path.

set -eu -o pipefail

set -x

source common.sh

# Avoid store dir being inside sandbox build-dir
unset NIX_STORE_DIR
unset NIX_STATE_DIR

setupStoreDirs

initLowerStore

mountOverlayfs

# A third store, so we can produce the very same input-addressed output path
# without either layer of the overlay having it yet.
storeC="$storeVolume/store-c"
mkdir -p "$storeC/nix/store"

drvPath=$(nix-instantiate --store "$storeA" ./lower-gains-invisible-path.nix --arg busybox "$busybox" --arg seed 1)
# Instantiating into the third store rather than copying the derivation there
# also asserts what the whole test rests on: the same derivation names the same
# input-addressed output path in an unrelated store.
drvPathC=$(nix-instantiate --store "$storeC" ./lower-gains-invisible-path.nix --arg busybox "$busybox" --arg seed 1)
[[ "$drvPathC" == "$drvPath" ]]

outPath=$(nix-store --store "$storeC" --realise "$drvPath")
[[ -n "$outPath" ]]

mergedPath="$(toRealPath "$storeBRoot/nix/store" "$outPath")"

# Seed the negative dentry: look the path up through the overlay while it is
# genuinely absent from both layers. This is the whole setup -- everything a
# build does to that path before producing it has the same effect.
[[ ! -e "$mergedPath" ]]

# ... and now the lower store gains it, with no remount.
nix copy --no-check-sigs --from "$storeC" --to "$storeA" "$outPath"
[[ -e "$(toRealPath "$storeA/nix/store" "$outPath")" ]]

# If this kernel shows the new lower entry anyway there is no divergence to
# test, and asserting anything about it would just be asserting the kernel.
if [[ -e "$mergedPath" ]]; then
  skipTest "this kernel exposes lower-store additions through a mounted overlay without a remount"
fi

# The store must not claim a path it cannot read. Before the fix this
# succeeded, and additionally wrote the lower store's registration into the
# upper database, so it kept succeeding for every later process too.
expect 1 nix-store --store "$storeB" --check-validity "$outPath"

# Which means asking for it builds it, rather than handing back a registration
# pointing at nothing.
builtPath=$(nix-store --store "$storeB" --realise "$drvPath")
[[ "$builtPath" == "$outPath" ]]

# The decisive check: the store object is now readable through the overlay.
# Before the fix, `--realise` handed back this same path having built nothing,
# and reading it failed.
[[ $(cat "$mergedPath") == 1 ]]

nix-store --store "$storeB" --check-validity "$outPath"
nix-store --store "$storeB" --verify-path "$outPath"

# And it is readable because this build put it in the upper layer, which the
# overlay shows in preference to the lower one -- not because the lower store's
# invisible copy somehow came back.
[[ -e "$storeBTop/$(basename "$outPath")" ]]
