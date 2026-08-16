#!/usr/bin/env bash

# A realisation's output path is a foreign key into the database of the store
# it is registered in: the insert fills `Realisations.outputPath` from a
# subselect over that store's own `ValidPaths`. The overlay store never made
# that row. `registerDrvOutput` went straight to the insert, so whenever the
# output path was one the lower layer had and this store had not copied up, the
# subselect yielded NULL and the operation died as
#
#   NOT NULL constraint failed: Realisations.outputPath
#
# naming neither the realisation nor the path.
#
# A lower-only output path is the normal state of a CI runner with a shared
# lower store and a per-job upper layer, where a sibling job or a substitution
# put the output there first. The copy-up that would have made the row lives in
# `isValidPathUncached`, and the two routes into `registerDrvOutput` skip it:
# the lower store's own realisation names a lower path by definition, and
# `Store::isValidPath` answers from the path-info cache, which
# `queryPathInfoUncached` fills from the lower store without registering
# anything.
#
# The driver is `test-register-realisation`, which registers a realisation and
# does nothing else. That is what makes this deterministic rather than a race:
# every CLI route into the same code incidentally calls `isValidPath` on the
# output path first, and that call performs the copy-up, so from a shell the
# bug is invisible. See the comment in register-realisation.cc.

set -eu -o pipefail

set -x

source common.sh

registerRealisation="${_NIX_TEST_BUILD_DIR?}/test-libstoreconsumer/test-register-realisation"
[[ -x "$registerRealisation" ]] \
  || fail "test-register-realisation was not built; the suite's meson 'deps' entry is wrong"

# Avoid store dir being inside sandbox build-dir
unset NIX_STORE_DIR
unset NIX_STATE_DIR

setupStoreDirs

initLowerStore

mountOverlayfs

# A third store, so outputs can exist without either overlay layer having built
# them. Content-addressed, so the very same bytes land at the very same path
# wherever they are produced.
storeC="$storeVolume/store-c"
mkdir -p "$storeC/nix/store"

buildInStoreC () {
  local seed=$1
  nix-instantiate --store "$storeC" ./lower-only-realisation.nix --arg busybox "$busybox" --arg seed "$seed"
}

drvBoth=$(buildInStoreC 1)
drvPathOnly=$(buildInStoreC 2)
drvNeither=$(buildInStoreC 3)

outBoth=$(nix-store --store "$storeC" --realise "$drvBoth")
outPathOnly=$(nix-store --store "$storeC" --realise "$drvPathOnly")
outNeither=$(nix-store --store "$storeC" --realise "$drvNeither")

# Case 1's output goes to the lower store with its realisation; case 2's goes
# there as a bare path, which carries no realisation. Case 3's goes nowhere.
nix copy --no-check-sigs --from "$storeC" --to "$storeA" "$drvBoth^out"
nix copy --no-check-sigs --from "$storeC" --to "$storeA" "$outPathOnly"
remountOverlayfs

# Both are readable through the overlay, so this is the visible-lower case and
# not the one `lower-gains-invisible-path.sh` covers.
[[ -e "$(toRealPath "$storeBRoot/nix/store" "$outBoth")" ]]
[[ -e "$(toRealPath "$storeBRoot/nix/store" "$outPathOnly")" ]]

# `$storeBRoot` opened as a plain local store is the overlay's own database and
# nothing else: no lower layer to fall back to. That is how this test tells
# "registered here" apart from "the lower store knows about it".
upperHas () {
  nix-store --store "$storeBRoot" --check-validity "$1"
}

expect 1 upperHas "$outBoth"
expect 1 upperHas "$outPathOnly"
expect 1 upperHas "$outNeither"

# Case 1: the lower store has the realisation. `registerDrvOutput` copies it up
# first so as to merge rather than mask it -- and that realisation names a
# lower path by construction, which nothing in this process has so much as
# looked at. No cache state required; this one was always going to fail.
"$registerRealisation" "$storeC" "$storeB" "$drvBoth" out
upperHas "$outBoth"

# Case 2: the lower store has only the path. The registration is the caller's
# own, and it is a `queryPathInfo` earlier in the process -- here explicit,
# in a real build any closure walk -- that leaves the path-info cache
# answering "valid" for a path this database has no row for.
"$registerRealisation" "$storeC" "$storeB" "$drvPathOnly" out --warm-cache
upperHas "$outPathOnly"

# Both realisations now resolve through the overlay, which is what registering
# them was for. The derivations go over only now, so that nothing about them
# could have made the output paths valid here before the registrations above.
nix copy --no-check-sigs --from "$storeC" --to "$storeB" "$drvBoth" "$drvPathOnly"
nix realisation info --store "$storeB" "$drvBoth^out" | grepQuiet "$outBoth"
nix realisation info --store "$storeB" "$drvPathOnly^out" | grepQuiet "$outPathOnly"

# Registering the same realisation twice must merge into the existing row
# rather than conflict.
"$registerRealisation" "$storeC" "$storeB" "$drvBoth" out

# Case 3: no layer has the output path, so there is nothing to copy up and the
# registration cannot be honoured. It must say so. Before the fix this was the
# raw SQLite constraint string, which named neither the realisation nor the
# path and read like database corruption.
# Nix bakes the terminal escapes into the message when it builds it, so strip
# them before matching or the pattern is being tested against a string nothing
# ever wrote.
expectStderr 1 "$registerRealisation" "$storeC" "$storeB" "$drvNeither" out \
  | sed 's/\x1b\[[0-9;]*m//g' > "$TEST_ROOT/register-neither.txt"
cat "$TEST_ROOT/register-neither.txt"
grepQuiet "cannot register realisation" "$TEST_ROOT/register-neither.txt"
grepQuiet "is not valid in this store" "$TEST_ROOT/register-neither.txt"
grepQuiet "$outNeither" "$TEST_ROOT/register-neither.txt"
expect 1 upperHas "$outNeither"
