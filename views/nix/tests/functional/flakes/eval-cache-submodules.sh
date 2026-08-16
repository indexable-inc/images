#!/usr/bin/env bash

# The fingerprint of a dirty git working directory has to account for its
# submodules, and only for the ones the input actually mounts.
#
# Both halves matter and they pull in opposite directions:
#
#   Too little. Submodules once aborted the fingerprint entirely, and without a
#   fingerprint `fetchToStore` skips its cache and `openEvalCache` opens
#   nothing, so a dirty checkout re-hashed and re-copied its whole source tree
#   into the store on every single evaluation (indexable-inc/index#4301).
#
#   Too much. When the input does not set `submodules`, the accessor renders
#   submodules as empty directories, so their contents cannot reach the
#   evaluation result. Hashing them anyway would invalidate the cache on edits
#   that provably cannot change the answer.
#
# What is observed is the fingerprint itself, via `nix flake metadata --json`,
# rather than a timing. "The cache was silently not used" costs only wall clock,
# which a test cannot assert on, and a fingerprint that fails to change costs
# correctness, which no timing would reveal either.
#
# When the fingerprint is missing entirely, the `nix` invocation fails before
# the assertion below can run its own comparison, because the test framework
# sets `_NIX_TEST_BARF_ON_UNCACHEABLE=1` and an unfingerprintable source path is
# uncacheable. So a regression here surfaces as an error from
# `nix flake metadata` on the `dirtyRootFp=` line rather than as the message
# attached to the check underneath it.

source ./common.sh

requireGit

subDir="$TEST_ROOT/fingerprint-sub"
rootDir="$TEST_ROOT/fingerprint-root"

createGitRepo "$subDir"
cat >"$subDir/flake.nix" <<EOF
{ outputs = _: { sub = "sub"; }; }
EOF
git -C "$subDir" add flake.nix
git -C "$subDir" commit -m "sub init"

createGitRepo "$rootDir"
cat >"$rootDir/flake.nix" <<EOF
{ outputs = _: { probe = "probe"; }; }
EOF
git -C "$rootDir" add flake.nix
git -C "$rootDir" commit -m "root init"
git -C "$rootDir" -c protocol.file.allow=always submodule add "$subDir" sub
git -C "$rootDir" commit -m "add submodule"

# Empty rather than absent when there is no fingerprint, so that "no
# fingerprint at all" and "a fingerprint that did not change" are distinct
# failures below instead of both showing up as equality.
fingerprintOf() {
    nix flake metadata "$1" --json | jq -r '.fingerprint // ""'
}

withSubs="git+file://$rootDir?submodules=1"
withoutSubs="git+file://$rootDir"

# A clean tree resolves to its HEAD commit and takes a different code path
# entirely, so this is a check on the test setup, not on the fix.
cleanFp=$(fingerprintOf "$withSubs")
[[ -n $cleanFp ]] || fail "no fingerprint for a clean tree with submodules"

# Dirty the top-level tree. This is the case that used to produce no
# fingerprint whatsoever.
echo '# dirty root' >>"$rootDir/flake.nix"
dirtyRootFp=$(fingerprintOf "$withSubs")
[[ -n $dirtyRootFp ]] || fail "a dirty tree with submodules got no fingerprint"
[[ $dirtyRootFp != "$cleanFp" ]] || fail "dirtying the top-level tree did not change the fingerprint"
git -C "$rootDir" checkout -- flake.nix

# Now the property the recursion exists for: with the top-level tree clean, an
# edit inside a mounted submodule must still change the fingerprint. Nothing
# but the recursion can observe this, and if it is missed the cache serves a
# stale result, which is worse than never caching.
echo '# dirty submodule' >>"$rootDir/sub/flake.nix"
dirtySubFp=$(fingerprintOf "$withSubs")
[[ -n $dirtySubFp ]] || fail "a dirty submodule got no fingerprint"
[[ $dirtySubFp != "$cleanFp" ]] \
    || fail "editing a mounted submodule did not change the fingerprint (stale cache hits)"

# Two different submodule contents must also differ from each other, so that
# the first edit was not merely hashed as "some submodule is dirty".
echo '# dirty submodule again' >>"$rootDir/sub/flake.nix"
dirtySubFp2=$(fingerprintOf "$withSubs")
[[ $dirtySubFp2 != "$dirtySubFp" ]] || fail "two different submodule contents shared one fingerprint"

# The opposite direction. Without `submodules`, the submodule is rendered as an
# empty directory and cannot affect the evaluation result, so its contents must
# not affect the fingerprint either. The submodule is still dirty here.
unmountedFp=$(fingerprintOf "$withoutSubs")
[[ -n $unmountedFp ]] || fail "no fingerprint for an unmounted-submodule input"
git -C "$rootDir/sub" checkout -- flake.nix
unmountedCleanFp=$(fingerprintOf "$withoutSubs")
[[ $unmountedFp == "$unmountedCleanFp" ]] \
    || fail "submodule contents changed the fingerprint of an input that does not mount them"
