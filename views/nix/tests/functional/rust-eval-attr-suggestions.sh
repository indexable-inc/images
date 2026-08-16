#!/usr/bin/env bash

# Did-you-mean suggestions on an attribute-not-found are the same on both
# evaluator arms, and enumerating a large set to build them is not the
# quadratic walk it used to be (ENG-12913).
#
# The bug this guards was invisible in the output: under `eval-backend = rust`
# a typo'd attribute on nixpkgs' 25,442-name top level printed exactly the
# message cppnix prints, and took 42 seconds to do it against cppnix's 2,
# because the bridge fetched the candidate names one index at a time through
# an accessor that rebuilt and re-sorted the whole name list per call. Nothing
# about the answer changed when it broke, so the cost guard lives in the crate
# (`enumerating_a_large_set_is_one_pass_not_one_per_name`) where it is cheap
# and hermetic. What this test holds is the other half: that the faster
# enumeration still feeds the suggestion machinery the same names, so the two
# arms say the same thing.
#
# Byte comparison is the right bar here despite error wording being tier 2
# (CLAUDE.md, "Parity bar"): both arms reach the SAME C++ code, cppnix's
# `Suggestions::bestMatches`, from the same `StringSet`. There is no second
# implementation to diverge in wording, so anything but identical output means
# the name set differs, which is a semantic difference wearing prose clothes.

source common.sh

rustArm=$'extra-experimental-features = rust-eval\neval-backend = rust\n'

# Same probe as rust-eval-refusal-token.sh: the binary may have been built
# without the Rust evaluator at all (-Dnix:rust-eval=disabled is the default),
# and only an answered evaluation is evidence that the arm exists.
if [[ "$(NIX_CONFIG=$rustArm nix-instantiate --eval --strict -E 1 2>&1)" != 1 ]]; then
    skipTest "this nix was built without the rust evaluator"
fi

# Compare the two arms on one selection, and fail printing the difference.
# Both streams are captured: a suggestion list is stderr, but an arm that
# wrongly SUCCEEDS would show up only on stdout.
diffArms() { # LABEL EXPR ATTR
    local label=$1 expr=$2 attr=$3
    local cppOut=$TEST_ROOT/$label.cpp.out cppErr=$TEST_ROOT/$label.cpp.err
    local rustOut=$TEST_ROOT/$label.rust.out rustErr=$TEST_ROOT/$label.rust.err

    if nix-instantiate --eval --strict -E "$expr" -A "$attr" > "$cppOut" 2> "$cppErr"; then
        echo "$label: the cpp arm did not fail, it printed:" >&2; cat "$cppOut" >&2; exit 1
    fi
    if env NIX_CONFIG="$rustArm" nix-instantiate --eval --strict -E "$expr" -A "$attr" \
            > "$rustOut" 2> "$rustErr"; then
        echo "$label: the rust arm did not fail, it printed:" >&2; cat "$rustOut" >&2; exit 1
    fi

    # The diff goes to stderr with the verdict. meson --print-errorlogs shows a
    # failing test's stderr and drops its stdout, so a diff written the usual
    # way leaves CI reporting that the arms differ without saying how.
    diffStream() { # WHICH CPPFILE RUSTFILE
        diff -u "$2" "$3" >&2 || {
            echo "$label: $1 differs between arms (cpp is -, rust is +)" >&2
            exit 1
        }
    }
    diffStream stderr "$cppErr" "$rustErr"
    diffStream stdout "$cppOut" "$rustOut"
}

# 1. The ordering case. cppnix keeps suggestions in a std::set<Suggestion>
# ordered by distance first and name second, then trims to 5 within distance
# 2, so the rendered list is distance-major. This set is built so that the two
# orders disagree: `fooba` is at distance 2 and sorts alphabetically before
# `fox` and `xoo`, which are at distance 1, so a name-ordered list and a
# distance-ordered one are different strings. Asserting only that the same
# NAMES appear would pass on both, which is why this is a byte comparison.
ordered='{ fo = 1; fooo = 2; fox = 3; xoo = 4; fooba = 5; unrelated = 6; }'
diffArms ordered "$ordered" foo

# The expected order, spelled out, so this test states cppnix's rule rather
# than only agreeing with whatever cppnix currently does. Without this the
# pair could drift together and still pass.
grepQuiet -F 'Did you mean one of fo, fooo, fox, xoo or fooba?' "$TEST_ROOT/ordered.cpp.err"

# 2. Nothing close enough. Everything here is past distance 2, so both arms
# must print the bare message with no suggestion clause at all -- the case
# where the enumeration happens and then produces nothing, which is what
# nixpkgs does for most typos and what made the bug's cost so surprising.
diffArms nomatch '{ alpha = 1; beta = 2; gamma = 3; }' zzzznotarealname
grepQuietInverse -F 'Did you mean' "$TEST_ROOT/nomatch.cpp.err"

# 3. At scale, which is the size the bug needed to be visible. 5,000 names is
# far past the point where a per-name rebuild stops being free, and it also
# exercises the buffer the names now cross in: one allocation holding 5,000
# NUL-terminated names rather than 5,000 separate crossings.
big='builtins.listToAttrs (builtins.genList (i: { name = "attr" + toString i; value = i; }) 5000)'
diffArms big "$big" attr9999x
grepQuiet -F "attribute 'attr9999x' in selection path 'attr9999x' not found" "$TEST_ROOT/big.cpp.err"
# A near miss on the same large set, so the scale case covers a rendered
# suggestion list and not only the empty one.
diffArms bignear "$big" attr499x
grepQuiet -F 'Did you mean' "$TEST_ROOT/bignear.cpp.err"

# 4. Names the buffer could mangle. The names cross packed back to back
# separated by NUL, so a name containing a space, a newline or a multi-byte
# character is where a splitter that guessed at the delimiter would show up,
# and an empty name is where an off-by-one would.
awkward='{ "" = 1; "a b" = 2; "c\nd" = 3; "é" = 4; }'
diffArms awkward "$awkward" ab

echo "rust-eval-attr-suggestions: ok"
