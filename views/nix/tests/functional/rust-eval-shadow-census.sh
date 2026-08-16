#!/usr/bin/env bash

# The shadow census survives a divergence whose text the JSON writer dislikes,
# and says so when it had to change anything (ENG-12874).
#
# The bug this pins was a byte-truncation: `shadowTruncate` cut a divergence
# detail at 200 bytes, and when byte 200 landed inside a multi-byte character
# the result was invalid UTF-8. `json::dump` refuses a document containing one,
# and the refusal took the whole document, so NIX_SHOW_STATS wrote ZERO BYTES
# for the process: attempts, verdicts, refusal tokens and every other
# divergence, not just the damaged row. A harness reads that as "nothing to
# report". Seven of 2638 attributes in a nixpkgs sweep went that way.
#
# So there are two claims here and the second is the important one:
#
#   1. the truncation cuts on a character boundary now;
#   2. and the writer cannot lose a census to one bad string ANYWAY, whatever
#      put the odd bytes there, because the trigger is not the bug and the
#      next one will not be a truncation.
#
# Both arms of one binary in the shape rust-eval-path-to-store.sh uses, except
# that this one needs `eval-backend = shadow`, which is the only setting under
# which the census exists at all.

source common.sh

clearStoreIfPossible

shadowArm=$'extra-experimental-features = rust-eval\neval-backend = shadow\n'

# As in the sibling tests: `nix config show` reports eval-backend on a binary
# compiled without the Rust evaluator, so ask by evaluating.
if [[ "$(NIX_CONFIG=$shadowArm nix-instantiate --eval --strict -E 1 2>&1)" != 1 ]]; then
    skipTest "this nix was built without the rust evaluator"
fi

work=$TEST_ROOT/shadow-census
rm -rf "$work"
mkdir -p "$work"

# `builtins.substring` on a set diverges (cpp reports a type error naming the
# set, the Rust arm reports its own), and cppnix renders the set into the
# message, so the payload below reaches the divergence detail. That is what
# lets this test choose which byte lands at the cut.
census() { # EXPR -> writes $work/stats.json and $work/err, prints the byte count
    rm -f "$work/stats.json"
    NIX_CONFIG="$shadowArm" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$work/stats.json" \
        nix-instantiate --eval --strict -E "$1" > /dev/null 2> "$work/err" || true
    wc -c < "$work/stats.json" | tr -d ' '
}

# The one-line assertion the whole file exists for. A census that counted an
# attempt must not serialise to nothing, and it must parse.
censusIsIntact() { # LABEL
    local label=$1 size attempts
    size=$(wc -c < "$work/stats.json" | tr -d ' ')
    if [[ $size -eq 0 ]]; then
        echo "$label: the census serialised to ZERO BYTES; a run that counted something reported nothing" >&2
        return 1
    fi
    attempts=$(jq -r '.shadow.attempts' < "$work/stats.json") || {
        echo "$label: the census is not parseable JSON" >&2
        return 1
    }
    [[ $attempts -ge 1 ]] || {
        echo "$label: expected at least one shadow attempt, got $attempts" >&2
        return 1
    }
    [[ $(jq -r '.shadow.divergences | length' < "$work/stats.json") -ge 1 ]] || {
        echo "$label: the divergence this test provokes is missing from the census" >&2
        return 1
    }
}

# 1. The truncation trigger, swept across the cut.
#
#    U+2018 and U+2019 are three bytes each, and whether one of them straddles
#    byte 200 depends on how much text precedes it -- which is why only 7 of
#    about 90 otherwise identical unfree throws were affected in the sweep that
#    found this. Padding lengths 143, 144, 152 and 153 were measured to zero
#    the census before the fix; the range is swept rather than those four named
#    so that a change in the error message's prefix moves which pad trips it
#    without moving this test off the boundary.
for pad in $(seq 138 158); do
    payload=$(printf 'a%.0s' $(seq 1 "$pad"))
    expr="builtins.substring 1 2 { x = \"${payload}‘unfree’ and then a tail long enough that the two hundredth byte falls inside that quote\"; }"
    census "$expr" > /dev/null
    censusIsIntact "pad=$pad"
    # The detail is still truncated, and still says how much it dropped: the
    # boundary fix must not have turned into "stop truncating".
    jq -e '.shadow.divergences[0].detail | test("bytes\\)")' < "$work/stats.json" > /dev/null || {
        echo "pad=$pad: the detail lost its truncation marker" >&2
        exit 1
    }
    # And the census needed no repairing. This is the assertion that makes the
    # boundary fix load-bearing rather than redundant: the writer guard below
    # would rescue a mid-character cut too, so without this line reverting
    # `shadowTruncate` to `substr(0, 200)` leaves this whole file green. The
    # two fixes are independent and each needs its own guard.
    jq -e 'has("serialisationDamage") | not' < "$work/stats.json" > /dev/null || {
        echo "pad=$pad: the truncation produced bytes the writer had to repair" >&2
        jq -c '.serialisationDamage' < "$work/stats.json" >&2
        exit 1
    }
done

# 2. The writer guard, with invalid UTF-8 that no truncation produced.
#
#    A lone 0xE2 read out of a file and forced into the error text. It is 12
#    bytes long, so `shadowTruncate` never touches it and the boundary fix
#    cannot be what saves the census here. Only the writer can.
printf 'lone \xe2 byte' > "$work/bad"
census "let s = builtins.readFile $work/bad; in builtins.seq s (builtins.substring 1 2 { x = s; })" > /dev/null
censusIsIntact "untruncated invalid utf-8"

# The damage is named in-band, so a CI gate reading the JSON alone cannot
# mistake a repaired census for a clean one.
jq -e '.serialisationDamage.fields | index("/shadow/divergences/0/detail")' < "$work/stats.json" > /dev/null || {
    echo "the repaired census does not name the field it repaired" >&2
    jq -c '.serialisationDamage' < "$work/stats.json" >&2
    exit 1
}
# And out of band, for a run nobody parses.
#
# `-a` because this stream contains the invalid byte by construction: the C++
# error quotes the offending file's contents verbatim, so stderr is genuinely
# not valid UTF-8 here. BSD grep (macOS) then classifies it as binary and
# reports no match at all, while GNU grep still counts it -- so without this
# the assertion passes on Linux and fails on Darwin for a reason that has
# nothing to do with what it is testing.
grepQuiet -a "eval statistics: 1 field" < "$work/err"

# 3. The negative: a clean run must NOT claim damage, or the field above is
#    decoration rather than a signal.
census 'builtins.substring 1 2 { x = "plain ascii"; }' > /dev/null
censusIsIntact "clean run"
jq -e 'has("serialisationDamage") | not' < "$work/stats.json" > /dev/null || {
    echo "a census with nothing wrong with it reported damage" >&2
    exit 1
}

echo "rust-eval-shadow-census: ok"
