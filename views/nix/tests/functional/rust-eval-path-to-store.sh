#!/usr/bin/env bash

# A path interpolated into a string is the store path cppnix copies it to,
# with the file actually in the store, and not the source path (ENG-12447).
#
# This test exists because the lang corpus cannot see that bug. `lang.sh` runs
# with `NIX_REMOTE=dummy://` and its only path interpolations are two files
# that do not exist, so the backend returning the source path showed up as two
# error-class mismatches while the case that matters -- a path that DOES exist,
# where the backend succeeded and the value was wrong -- was untested. Here the
# store is the test's own real store, so a wrong value is a failed assertion
# rather than an invisible one.
#
# Both arms of one binary, cpp as the oracle, in the shape lang-diff.sh uses.

source common.sh

clearStoreIfPossible

rustArm=$'extra-experimental-features = rust-eval\neval-backend = rust\n'

# The binary may have been built without the Rust evaluator at all
# (-Dnix:rust-eval=disabled is the default). `nix config show` reports
# eval-backend either way, so ask by evaluating: a probe that answers 1 is the
# only evidence the arm exists. Skipping is right here and refusing is right in
# lang-diff.sh, because that harness is run deliberately against a rust build
# and this one runs in every `meson test`.
if [[ "$(NIX_CONFIG=$rustArm nix-instantiate --eval --strict -E 1 2>&1)" != 1 ]]; then
    skipTest "this nix was built without the rust evaluator"
fi

srcDir=$TEST_ROOT/interp
mkdir -p "$srcDir"
echo -n 'the contents decide the store path' > "$srcDir/f"

# --read-write-mode because the default for --eval is read-only, where cppnix
# computes the store path the copy WOULD produce and moves no bytes. Both are
# worth testing and the copying one is the stronger claim, so it goes first.
both() { # EXPR -> prints "<cpp output>|<rust output>", refuses if they differ
    local expr=$1
    shift
    local cpp rust
    cpp=$(nix-instantiate --eval --strict "$@" -E "$expr")
    rust=$(NIX_CONFIG=$rustArm nix-instantiate --eval --strict "$@" -E "$expr")
    if [[ $cpp != "$rust" ]]; then
        echo "backends disagree on $expr" >&2
        echo "  cpp:  $cpp" >&2
        echo "  rust: $rust" >&2
        return 1
    fi
    printf '%s\n' "$cpp"
}

# 1. The value is a store path, on both arms, byte for byte.
out=$(both "\"\${$srcDir/f}\"" --read-write-mode)
[[ $out =~ ^\"$NIX_STORE_DIR/[0-9a-z]{32}-f\"$ ]] || {
    echo "not a store path: $out" >&2
    exit 1
}
# The negative that the cheap fix would pass: it must not be the source path.
[[ $out != "\"$srcDir/f\"" ]]

# 2. The file is really in the store under that name, so this is a copy and
#    not a computed string.
storePath=${out%\"}
storePath=${storePath#\"}
[[ $(cat "$storePath") == 'the contents decide the store path' ]]

# 3. The store path is content-addressed: edit the file, get a different one.
echo -n 'different contents' > "$srcDir/f"
other=$(both "\"\${$srcDir/f}\"" --read-write-mode)
[[ $other != "$out" ]]

# 4. Read-only mode (the default for --eval, and what the corpus runs under)
#    answers with the same path without copying. Same expression, fresh store.
clearStoreIfPossible
readOnly=$(both "\"\${$srcDir/f}\"")
[[ $readOnly == "$other" ]]
[[ ! -e ${readOnly//\"/} ]]

# 5. Interpolation is not toString: cppnix passes copyToStore only for the
#    former, so a backend that copied in both would also be wrong.
plain=$(both "builtins.toString $srcDir/f")
[[ $plain == "\"$srcDir/f\"" ]]

# 6. A path on the LEFT of + stays a path and copies nothing; a string on the
#    left makes the right-hand path a store copy, exactly as interpolation does.
[[ $(both "$srcDir/f + \"/g\"") == "$srcDir/f/g" ]]
[[ $(both "\"\" + $srcDir/f") == "$other" ]]

# 7. A path that does not exist fails on both arms. This is the case the corpus
#    already had; it is here so the two halves of the mechanism sit together.
expectStderr 1 nix-instantiate --eval --strict -E "\"\${$srcDir/nope}\"" \
    | grepQuiet "does not exist"
expectStderr 1 env NIX_CONFIG="$rustArm" nix-instantiate --eval --strict -E "\"\${$srcDir/nope}\"" \
    | grepQuiet "does not exist"

echo "rust-eval-path-to-store: ok"
