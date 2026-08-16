#!/usr/bin/env bash

# Write-through publication sent the outputs and not the realisations.
#
# For an input-addressed output that is survivable: the output path is a
# function of the derivation, so a host that has the derivation can name the
# path and ask a cache for it. For a floating content-addressed output it is
# not. The path is a function of the bytes, which are the thing being asked
# for, so without the realisation -- the `.doi` object mapping the derivation's
# output to its path -- there is nothing to look up, and the host rebuilds.
# Every time, however warm the cache is.
#
# The cause is which overload the publication reached. `publishOutputs`
# collects output paths into a `StorePathSet` and copies their closure;
# `registerDrvOutput` on the destination is called only by the
# `RealisedPath::Set` overload of `copyPaths`, which nothing on this route
# used. The upper lane was unaffected because it publishes with
# `nix copy "<drv>^*"`, which goes through that overload.

source common.sh

TODO_NixOS

# `write-through-store` is read by whichever process runs the build, so through
# the daemon it would be the daemon's setting rather than the one under test.
needLocalStore "'write-through-store' is read by the process that performs the build"

enableFeatures "ca-derivations"

builderMarker="building-the-write-through-store-ca-fixture"

wtCache="$TEST_ROOT/wtcache-ca"
rm -rf "$wtCache"

clearStore

outPath=$(nix-build write-through-store-ca.nix --no-out-link \
    --option write-through-store "file://$wtCache")

# The NAR half was never broken; assert it anyway, so a failure below is about
# the realisation and not about publication having stopped working entirely.
outBase=$(basename "$outPath")
[[ -e "$wtCache/${outBase%%-*}.narinfo" ]]

# The realisation is the half that was missing. Count them rather than test a
# glob for existence, which an empty directory quietly satisfies.
shopt -s nullglob
realisations=("$wtCache"/realisations/*.doi)
shopt -u nullglob
if [[ ${#realisations[@]} -ne 1 ]]; then
    echo "expected exactly one published realisation under $wtCache/realisations," \
         "got ${#realisations[@]}: ${realisations[*]}" >&2
    exit 1
fi
# It has to name this build's output, not merely exist.
grepQuiet "$outBase" "${realisations[0]}"

# And what that buys, which is the whole point: a store holding only the
# derivation resolves the output from the cache instead of building it again.
clearStore

rebuildLog="$TEST_ROOT/write-through-ca-rebuild.log"
nix-build write-through-store-ca.nix --no-out-link \
    --substituters "file://$wtCache" --no-require-sigs \
    2> "$rebuildLog"
[[ -e $outPath ]]
# Both directions: the builder did not run, and the path arrived from the
# cache. The absence on its own would also be satisfied by a `nix-build` that
# did nothing at all.
grepQuietInverse "$builderMarker" "$rebuildLog"
grepQuiet 'copying path' "$rebuildLog"

# An impure derivation is deliberately excluded: its result is a one-off, so
# publishing a realisation for it would tell every other host that this run's
# output is what the derivation produces. `registerOutputs` does not register
# one locally either, and the two decisions are the same predicate.
