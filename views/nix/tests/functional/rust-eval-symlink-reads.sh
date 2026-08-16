#!/usr/bin/env bash

# The filesystem reads follow symlinks exactly where cppnix follows them, and
# refuse exactly where cppnix refuses (ENG-12871).
#
# Most of that is pinned by lang corpus pairs, which are the better home
# because lang-diff scores them against the cpp arm live. Two things cannot
# live there and are the reason this file exists:
#
#  1. `pure-eval`. A corpus case cannot set it, because under `pure-eval`
#     nix-instantiate cannot read the `.nix` file it was pointed at -- the
#     refusal names the corpus file itself and the expression never runs. It
#     matters here because the whole point of resolving through cppnix's
#     `rootFS` rather than with a `realpath` of our own is that the allow list
#     applies to the resolution, so it needs an assertion.
#
#  2. Invalidation. The corpus evaluates each case once. Resolution changes
#     WHICH file a read set's answer came from, so an edit to the symlink and
#     an edit to its target must each invalidate a memoised result, and only a
#     second evaluation can show that.
#
# Both arms of one binary, cpp as the oracle, in the shape
# rust-eval-path-to-store.sh uses.

source common.sh

clearStoreIfPossible

rustArm=$'extra-experimental-features = rust-eval\neval-backend = rust\n'

# As in rust-eval-path-to-store.sh: `nix config show` reports eval-backend on a
# binary compiled without the Rust evaluator, so ask by evaluating.
if [[ "$(NIX_CONFIG=$rustArm nix-instantiate --eval --strict -E 1 2>&1)" != 1 ]]; then
    skipTest "this nix was built without the rust evaluator"
fi

tree=$TEST_ROOT/symlinks
rm -rf "$tree"
mkdir -p "$tree/dir"
echo -n 'target contents' > "$tree/target"
echo -n 'other contents' > "$tree/other"
echo '1' > "$tree/dir/default.nix"
ln -s target "$tree/link"
ln -s dir "$tree/link-to-dir"
ln -s nowhere "$tree/dangling"

both() { # EXPR -> prints the agreed output, refuses if the arms differ
    local expr=$1 cpp rust
    cpp=$(nix-instantiate --eval --strict -E "$expr")
    rust=$(NIX_CONFIG=$rustArm nix-instantiate --eval --strict -E "$expr")
    if [[ $cpp != "$rust" ]]; then
        echo "backends disagree on $expr" >&2
        echo "  cpp:  $cpp" >&2
        echo "  rust: $rust" >&2
        return 1
    fi
    printf '%s\n' "$cpp"
}

bothFail() { # EXPR NEEDLE -> both arms fail and both say NEEDLE
    local expr=$1 needle=$2
    expectStderr 1 nix-instantiate --eval --strict -E "$expr" | grepQuiet "$needle"
    expectStderr 1 env NIX_CONFIG="$rustArm" nix-instantiate --eval --strict -E "$expr" \
        | grepQuiet "$needle"
}

# 1. Following, per primop. readFile and readDir resolve the leaf; pathExists
#    resolves ancestors only, which is why a dangling link exists; readFileType
#    resolves nothing at all.
[[ $(both "builtins.readFile $tree/link") == '"target contents"' ]]
[[ $(both "builtins.readDir $tree/link-to-dir") == '{ "default.nix" = "regular"; }' ]]
[[ $(both "builtins.readFile $tree/link-to-dir/default.nix") == '"1\n"' ]]
[[ $(both "builtins.pathExists $tree/dangling") == 'true' ]]
[[ $(both "builtins.readFileType $tree/link") == '"symlink"' ]]
[[ $(both "import $tree/link-to-dir") == '1' ]]

# 2. Refusing. A dangling link reports the missing TARGET, because the
#    resolution ran and then the read failed; readFileType reports the
#    ancestor as a symlink, because no resolution ran at all.
bothFail "builtins.readFile $tree/dangling" "$tree/nowhere' does not exist"
bothFail "builtins.readFileType $tree/link-to-dir/default.nix" "$tree/link-to-dir' is a symlink"

# 3. pure-eval, and the assertion is WHICH path the refusal names.
#
#    cppnix's rootFS is wrapped in an AllowListSourceAccessor when either
#    purity setting is on (eval.cc:306), and SourceAccessor::resolveSymlinks
#    walks the path by calling maybeLstat and readLink on that same accessor
#    (source-accessor.cc:113). So the allow list applies DURING the
#    resolution, and the first component it refuses is the one it names: the
#    symlink, never the target it would have resolved to.
#
#    That is the difference between resolving through the accessor and
#    resolving with a realpath of our own, and it is the only observable one.
#    A resolution done outside the accessor would name the target here, or
#    reach it.
for setting in pure-eval restrict-eval; do
    for arm in '' "$rustArm"; do
        expectStderr 1 env NIX_CONFIG="$arm" nix-instantiate --eval --strict \
            --option "$setting" true -E "builtins.readFile $tree/link" \
            | grepQuiet "$tree/link' is forbidden"
        # The negative: naming the target would mean the resolution happened
        # somewhere the allow list is not.
        expectStderr 1 env NIX_CONFIG="$arm" nix-instantiate --eval --strict \
            --option "$setting" true -E "builtins.readFile $tree/link" \
            | grepQuietInverse "$tree/target"
    done
done

# 4. Invalidation, with the memo table on. Resolution means the answer to
#    "read $tree/link" comes from a file whose name is not in the question, so
#    an edit to EITHER end of the link has to change the answer. It does
#    because the witness replays the question rather than the recorded answer,
#    and replaying it resolves again -- but that is an argument, and this is
#    the measurement.
cacheDir=$TEST_ROOT/symlink-cache
rm -rf "$cacheDir"
cached() { # EXPR -> the rust arm's answer with the memo table on
    NIX_CONFIG="$rustArm"$'eval-cache-dir = '"$cacheDir"$'\n' \
        nix-instantiate --eval --strict -E "$1"
}

[[ $(cached "builtins.readFile $tree/link") == '"target contents"' ]]
# 4a. Edit the target the link points at. Same question, same link, new answer.
echo -n 'edited target' > "$tree/target"
[[ $(cached "builtins.readFile $tree/link") == '"edited target"' ]]
# 4b. Repoint the link, leaving both files alone. Same question again.
ln -sfn other "$tree/link"
[[ $(cached "builtins.readFile $tree/link") == '"other contents"' ]]
# 4c. And back, to show 4b was the link moving rather than a cache that never
#     hits: this returns to an answer the cache has already seen.
ln -sfn target "$tree/link"
[[ $(cached "builtins.readFile $tree/link") == '"edited target"' ]]

echo "rust-eval-symlink-reads: ok"
