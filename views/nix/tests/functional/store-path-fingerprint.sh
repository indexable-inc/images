#!/usr/bin/env bash

# A source path inside the store is cacheable. The store object it belongs to
# is immutable once registered, so the NAR hash the store already recorded for
# it is a fingerprint, and `fetchToStore` can answer from its cache without
# reading a single file.
#
# Without a fingerprint `fetchToStore` skips its cache entirely, so every
# evaluation that reaches a path under the store dumps the whole enclosing
# store object through a NAR sink and hashes it again. On one real
# configuration that was 105 MB and 7,630 files re-hashed on every eval in
# order to consume 12 KB of it (indexable-inc/index#4323).
#
# What is asserted below is not the speedup, which a test cannot observe, but
# the two ways a fingerprint can be wrong, which pull in opposite directions:
#
#   Too coarse. Two different contents share one fingerprint, so the cache
#   answers for the wrong bytes and evaluation gets a store path that does not
#   hold what it claims. That is a correctness bug and much worse than a slow
#   eval, so it gets the most checks here.
#
#   Too fine. The fingerprint changes when the content did not, the cache never
#   hits, and nothing was fixed. `_NIX_TEST_BARF_ON_UNCACHEABLE=1` turns the
#   total absence of a fingerprint into a hard error rather than a silent
#   slowdown, and the `cache hit` greps catch the case where one exists but
#   never matches.

source common.sh

# Not adapted to the NixOS lane, where the store is the real /nix/store and the
# suite runs as an unprivileged user. The last assertion below plants
# unregistered content INSIDE the store directory, which needs a store this test
# owns: it reads $NIX_STORE_DIR, and common/vars.sh exports that only when
# `! isTestOnNixOS`, so under the suite's `bash -u` the reference is an unbound
# variable and the test dies right there. Pointing it at /nix/store instead would
# not help, because `mkdir` under a real store is not ours to do.
#
# This is a test-environment gap and NOT a hole in the fingerprint. Measured by
# hand on aarch64-darwin against a live daemon store with this same build: the
# cold eval of a store subpath copies once, the warm eval in a fresh process
# reports `cache hit in` on the same output path, and
# _NIX_TEST_BARF_ON_UNCACHEABLE=1 was set throughout and never fired. So the
# indexable-inc/index#4323 speedup does apply through a daemon store.
#
# Red in CI runs 30612815353 (f200a3a8d) and 30619033324 (60d1391a5), both
# `34/223 store-path-fingerprint FAIL` inside
# vm-test-run-functional-tests-on-nixos_user, while the same test passed in the
# component-test lane of those same jobs. The unbound variable is a certainty;
# whether the assertions before it would pass on that lane is untested, so this
# skip does not claim they would. Adapting the test means giving the
# unregistered-content case a store directory it owns.
TODO_NixOS

export _NIX_TEST_BARF_ON_UNCACHEABLE=1

stderrFile="$TEST_ROOT/fingerprint-stderr"

# `-vvvv` is what makes `fetchToStore`'s own account of what it did visible.
# Comparing output paths alone cannot tell a cache hit from a re-hash that
# happened to agree, and it is exactly the re-hash this change exists to avoid.
evalStoreSubpath() {
    nix eval --impure --raw -vvvv --expr "\"\${$1}\"" 2>"$stderrFile"
}

assertCacheHit() {
    grep -q "cache hit in" "$stderrFile" \
        || fail "$1: expected a fetcher cache hit; stderr said: $(grep -E "copied '|hashing '|uncacheable" "$stderrFile" || true)"
}

assertCacheMiss() {
    grep -q "copied '" "$stderrFile" \
        || fail "$1: expected a copy into the store, so the hit below proves the cache and not a warm store"
}

# Two store objects with IDENTICAL content, and a third that differs. The
# duplicate is the discriminating fixture: `nix store add-path` folds the name
# into the store path, so `dupA` and `dupB` are different paths holding the
# same bytes.
mkdir -p "$TEST_ROOT/tree/sub/inner"
echo one > "$TEST_ROOT/tree/sub/inner/a"
echo top > "$TEST_ROOT/tree/top"
dupA=$(nix store add-path "$TEST_ROOT/tree" --name fp-dup-a)
dupB=$(nix store add-path "$TEST_ROOT/tree" --name fp-dup-b)
[[ $dupA != "$dupB" ]] || fail "test setup: the two duplicate fixtures got one store path"

mkdir -p "$TEST_ROOT/tree2/sub/inner"
echo two > "$TEST_ROOT/tree2/sub/inner/a"
other=$(nix store add-path "$TEST_ROOT/tree2" --name fp-other)

# The answer a full walk would give, computed independently of the fetcher.
# `nix store add-path` and `fetchToStore` both name the result after the last
# path component and both address it by the NAR hash of the same bytes, so the
# two must agree. This is the check that a cache HIT returns the RIGHT hash,
# rather than merely returning quickly.
expectedSub=$(nix store add-path "$TEST_ROOT/tree/sub" --name sub)

# Cold: nothing in the fetcher cache yet, so this copies. It also proves the
# path is fingerprintable at all, because `_NIX_TEST_BARF_ON_UNCACHEABLE=1`
# would have made an unfingerprintable source path a hard error instead.
outA=$(evalStoreSubpath "$dupA/sub") \
    || fail "a store subpath is not fingerprintable: $(tail -3 "$stderrFile")"
assertCacheMiss "cold eval of a store subpath"
[[ $outA == "$expectedSub" ]] \
    || fail "a store subpath evaluated to '$outA', but hashing it directly gives '$expectedSub'"
[[ $(cat "$outA/inner/a") == one ]] || fail "the result of '$dupA/sub' does not hold its content"

# Warm, in a fresh process so the in-memory `srcToStore` map cannot be what
# answers. This is the whole point: no bytes are read and the cache answers.
outA2=$(evalStoreSubpath "$dupA/sub")
assertCacheHit "warm eval of the same store subpath"
[[ $outA2 == "$outA" ]] || fail "warm eval of '$dupA/sub' disagreed with the cold one"

# The fingerprint is the store object's CONTENT hash, not its store path. This
# is the first time `dupB` has ever been evaluated, so a cache hit here can
# only come from its content hash matching `dupA`'s. Keying on the store path
# instead would make this a miss, and would also be unsound: an
# input-addressed output path is a function of its derivation and not of its
# content, so a non-reproducible build re-run after a garbage collection can
# leave different bytes at the same path.
outB=$(evalStoreSubpath "$dupB/sub")
assertCacheHit "first-ever eval of a different store path holding identical content"
[[ $outB == "$outA" ]] \
    || fail "two store objects with identical content gave different results ('$outB' vs '$outA')"

# The opposite direction, and the one that would serve stale results if the
# fingerprint were too coarse: different content at the same subpath of a
# different store object must not reuse the entry.
outC=$(evalStoreSubpath "$other/sub")
assertCacheMiss "eval of a store subpath with different content"
[[ $outC != "$outA" ]] \
    || fail "different content shared one result ('$outC'), so the cache would serve stale bytes"
[[ $(cat "$outC/inner/a") == two ]] || fail "the result of '$other/sub' does not hold its content"
outC2=$(evalStoreSubpath "$other/sub")
assertCacheHit "warm eval of the differing store subpath"
[[ $outC2 == "$outC" ]] || fail "warm eval of '$other/sub' disagreed with the cold one"

# The root of a store object, not just a subpath of one, since the subpath is a
# separate component of the cache key and the root case exercises the empty one.
outRoot=$(evalStoreSubpath "$dupA")
[[ $outRoot != "$outA" ]] || fail "the root of a store object and its subdirectory gave one result"
outRoot2=$(evalStoreSubpath "$dupA")
assertCacheHit "warm eval of a whole store object"
[[ $outRoot2 == "$outRoot" ]] || fail "warm eval of '$dupA' disagreed with the cold one"

# Content that merely SITS under the store directory without being registered
# gets no fingerprint. Only a registered store object is immutable; an
# unregistered directory (a half-written path, a build's leftover chroot) can
# change under us and the store has recorded no hash to key on. Dropping this
# guard would silently start caching such content, which no assertion on the
# happy path above would notice, so it is checked by requiring the error.
bogus="$NIX_STORE_DIR/00000000000000000000000000000000-fp-bogus"
mkdir -p "$bogus/sub"
echo bogus > "$bogus/sub/a"
expectStderr 1 nix eval --impure --raw --expr "\"\${$bogus/sub}\"" \
    | grepQuiet "is uncacheable" \
    || fail "unregistered content under the store directory was given a fingerprint"
