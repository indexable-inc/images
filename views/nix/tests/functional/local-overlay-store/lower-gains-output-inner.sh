#!/usr/bin/env bash

# The lower store may gain store objects while an overlay over it is mounted --
# the manual asks only that it not change in other ways. So an output path can
# become valid *while the overlay store is building the derivation that
# produces it*: the overlay's own output locks say nothing about who writes the
# lower store. This used to abort the whole nix process in `registerOutputs`,
# on an assertion that only a content-addressed output could find its path
# already valid.
#
# Reproduce that without a real race: build the derivation in a third store,
# start the same build against the overlay, and register the third store's
# result in the lower store while the overlay's builder is still spinning.
#
# The derivation is deliberately non-reproducible, so the two builds differ.
# That pins the second half of the fix too: the overlay build must keep the
# *registered* path info rather than record its own hash against content it did
# not produce, which `--verify-path` would then reject.

set -eu -o pipefail

set -x

source common.sh

# Avoid store dir being inside sandbox build-dir
unset NIX_STORE_DIR
unset NIX_STATE_DIR

setupStoreDirs

initLowerStore

mountOverlayfs

# A third store, so the very same input-addressed output path can be produced
# without either layer of the overlay having it yet.
storeC="$storeVolume/store-c"
mkdir -p "$storeC/nix/store"

drvPath=$(nix-instantiate --store "$storeA" ./lower-gains-output.nix --arg busybox "$busybox" --arg seed 1)

# Instantiating into the third store rather than copying the derivation there
# also asserts what this whole test rests on: the same derivation names the
# same input-addressed output path in an unrelated store.
drvPathC=$(nix-instantiate --store "$storeC" ./lower-gains-output.nix --arg busybox "$busybox" --arg seed 1)
[[ "$drvPathC" == "$drvPath" ]]

outPath=$(nix-store --store "$storeC" --realise "$drvPath")
[[ -n "$outPath" ]]

# Neither overlay layer has the output yet, so the overlay really does build.
expect 1 nix-store --store "$storeB" --check-validity "$outPath"

buildLog="$TEST_ROOT/overlay-build.log"
buildOut="$TEST_ROOT/overlay-build.out"
nix-store --store "$storeB" --realise "$drvPath" > "$buildOut" 2> "$buildLog" &
buildPid=$!

# Wait for the builder itself to be live, rather than guessing at a sleep, so
# the window for the next step is as wide as the builder's spin allows.
started=false
for _ in $(seq 1 100); do
  if grepQuiet "BUILDER_STARTED" "$buildLog"; then
    started=true
    break
  fi
  sleep 0.2
done
"$started" || { kill "$buildPid" 2>/dev/null || true; skipTest "the overlay build never reported starting"; }

# ... and now the lower store gains exactly the path that build is producing.
nix copy --no-check-sigs --from "$storeC" --to "$storeA" "$outPath"
remountOverlayfs

# If the builder already finished, the branch under test was never reached, and
# a pass here would mean nothing.
if ! kill -0 "$buildPid" 2>/dev/null; then
  skipTest "the overlay build finished before the lower store could gain its output"
fi

# Before the fix this aborts: "Assertion 'newInfo.ca' failed".
wait "$buildPid"

[[ $(cat "$buildOut") == "$outPath" ]]

# The build is expected to notice it did not produce what is registered.
grepQuiet "may not be deterministic" "$buildLog"

# The decisive check: the overlay store's recorded hash for the path must
# describe the bytes that are actually there (the lower store's copy), not the
# ones this build made and threw away.
nix-store --store "$storeB" --verify-path "$outPath"

# And the content is the lower store's, not this build's.
[[ $(cat "$(toRealPath "$storeA/nix/store" "$outPath")") \
   == $(cat "$(toRealPath "$storeBRoot/nix/store" "$outPath")") ]]
