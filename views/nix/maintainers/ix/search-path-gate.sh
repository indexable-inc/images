#!/usr/bin/env bash
#
# Does `<x>` mean the same thing under both backends?
#
# Differential, like the other gates here: every case runs twice through one
# binary, once with `eval-backend = cpp` and once with `rust`, compared byte
# for byte on stdout and by exit code. ENG-12443.
#
# What it is really checking is that the desugaring is cppnix's. `<x>` is not
# a path literal: cppnix's parser turns it into `__findFile __nixPath "x"`, so
# a program that rebinds either name changes the lookup, and a backend that
# resolved `<x>` directly would be right about the common case and silently
# wrong about that one -- section 3 is that case, and it produces a value
# rather than an error, so nothing else here would catch it.
#
# Needs a built nix with the Rust evaluator linked in. Point it at one with
# NIX_BUILD_DIR; the default matches the other gates.
set -u

BUILD=${NIX_BUILD_DIR:-$HOME/incr-vm/nix/build}
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || { echo "no nix-instantiate at $NIXI (set NIX_BUILD_DIR)"; exit 2; }
# shellcheck source=./gate-ratchets.sh
. "$(cd "$(dirname "$0")" && pwd)/gate-ratchets.sh"
# shellcheck source=./compare-arms.sh
. "$(cd "$(dirname "$0")" && pwd)/compare-arms.sh" || exit 2

W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
mkdir -p "$W/dir1" "$W/dir2" "$W/dir3"
echo '"a-from-dir1"' > "$W/dir1/a.nix"
echo '"a-from-dir2"' > "$W/dir2/a.nix"
echo '"b-from-dir2"' > "$W/dir2/b.nix"
echo '"c-from-dir3"' > "$W/dir3/c.nix"

BASE="extra-experimental-features = rust-eval"
CPP="$BASE
eval-backend = cpp"
RUST="$BASE
eval-backend = rust"
INCL=(-I "$W/dir1" -I "$W/dir2" -I "pre=$W/dir3")

# A binary built without the Rust evaluator reports `eval-backend = rust` and
# runs the cpp one, which would make every case below a pair of identical cpp
# runs scoring a clean sheet. Probe by evaluating, as lang-diff.sh does.
probe=$(NIX_CONFIG="$RUST" "$NIXI" --eval --strict -E 1 2>&1)
[ "$probe" = "1" ] || { echo "search-path-gate: rust arm cannot evaluate '1' ($probe); refusing to score"; exit 2; }
# And the stronger form: which backend actually served it. Derived from a
# per-backend count of evaluations, not echoed back from the setting
# (ENG-12542), so a setting that parses and does nothing is caught here.
for arm in cpp rust; do
  case $arm in cpp) cfg=$CPP ;; *) cfg=$RUST ;; esac
  NIX_CONFIG="$cfg" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats-$arm.json" \
    "$NIXI" --eval --strict -E 1 > /dev/null 2>&1
  ev=$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))
except OSError:
    print("<no stats file>")' "$W/stats-$arm.json")
  [ "$ev" = "$arm" ] || {
    echo "search-path-gate: the $arm arm reports evaluator='$ev'; both arms would be the same backend and every comparison below would be vacuous"
    exit 2
  }
done

pairs=0; match=0; mismatch=0; refused=0; produced=0; empty=0; failed_alike=0
failures=()

same() {
  local label=$1 expr=$2
  pairs=$((pairs + 1))
  NIX_CONFIG="$CPP" "$NIXI" --eval --strict "${INCL[@]}" -E "$expr" > "$W/cpp.out" 2> "$W/cpp.err"
  local rc_cpp=$?
  NIX_CONFIG="$RUST" "$NIXI" --eval --strict "${INCL[@]}" -E "$expr" > "$W/rust.out" 2> "$W/rust.err"
  local rc_rust=$?

  if [ $rc_rust -ne 0 ] && grep -q "rust-eval unimplemented" "$W/rust.err"; then
    refused=$((refused + 1))
    echo "  REFUSED  $label -- $(grep -o 'rust-eval unimplemented: .*' "$W/rust.err" | head -1)"
    return 0
  fi
  arms_score "$W/cpp.out" "$rc_cpp" "$W/rust.out" "$rc_rust"
  if [ "$ARMS_VERDICT" = match ]; then
    match=$((match + 1))
    produced=$((produced + 1))
    echo "  ok       $label -- $(head -c 100 "$W/cpp.out")"
    return 0
  fi
  # Both arms rejected it identically. Two rows here are `tryEval` probes of
  # malformed input and are meant to, so this is a verdict and not a failure --
  # but it is its own count, because "both refused" and "both agreed on a
  # value" are different claims and the ratchet should not be able to trade
  # one for the other.
  if [ "$ARMS_VERDICT" = fail-both ]; then
    failed_alike=$((failed_alike + 1))
    echo "  both-fail $label -- exited $rc_cpp on each; cpp err=[$(head -c 90 "$W/cpp.err")]"
    return 0
  fi
  # Agreement with nothing to agree about: both arms exited 0 and printed
  # nothing. This used to count as `match` with `produced` left alone, so the
  # only thing between an empty row and a green gate was the `produced`
  # ratchet -- a floor, not a verdict, and one a second empty row could be
  # traded against.
  if [ "$ARMS_VERDICT" = empty ]; then
    empty=$((empty + 1))
    failures+=("$label (both arms succeeded and printed nothing)")
    echo "  EMPTY    $label -- both arms exited 0 and neither printed anything"
    echo "    cpp  err=[$(head -c 200 "$W/cpp.err")]"
    echo "    rust err=[$(head -c 200 "$W/rust.err")]"
    return 0
  fi
  mismatch=$((mismatch + 1))
  failures+=("$label")
  echo "  MISMATCH $label"
  echo "    cpp  rc=$rc_cpp out=[$(head -c 200 "$W/cpp.out")] err=[$(head -c 200 "$W/cpp.err")]"
  echo "    rust rc=$rc_rust out=[$(head -c 200 "$W/rust.out")] err=[$(head -c 200 "$W/rust.err")]"
}

echo "== 1. resolution =="
same "first -I wins"          '<a.nix>'
same "second -I when first misses" '<b.nix>'
same "prefixed entry"         '<pre/c.nix>'
same "import through <>"      'import <a.nix>'
same "type of a lookup"       'builtins.typeOf <a.nix>'

echo "== 2. the default list is a value =="
same "nixPath is a list"      'builtins.isList __nixPath'
same "entries have both keys" 'builtins.attrNames (builtins.head __nixPath)'
same "builtins spelling"      'builtins.isList builtins.nixPath'
same "the two agree"          '__nixPath == builtins.nixPath'
same "findFile directly"      'builtins.findFile __nixPath "a.nix"'

echo "== 3. the desugaring is a call, so both names are rebindable =="
# The case that separates cppnix's desugaring from resolving `<x>` directly.
# Both arms must answer dir2's file, not dir1's, and a backend that ignored
# the rebinding would answer "a-from-dir1" -- a value, not an error.
same "rebound __nixPath"      "let __nixPath = [ { path = \"$W/dir2\"; } ]; in import <a.nix>"
same "rebound to nothing"     'let __nixPath = []; in (builtins.tryEval <a.nix>).success'
# The ${n} below is Nix's interpolation and must reach the evaluator intact.
# shellcheck disable=SC2016
same "rebound __findFile"     'let __findFile = p: n: "hijacked ${n}"; in <a.nix>'
same "with does not shadow"   'with { __nixPath = []; }; builtins.isList __nixPath'

echo "== 4. a miss is a catchable throw =="
same "tryEval of a miss"      '(builtins.tryEval <nosuchentry>).success'
same "miss is not caught by ?" 'builtins.tryEval <nosuchentry>'

echo "== 5. malformed lists =="
same "entry without path"     '(builtins.tryEval (builtins.findFile [ { prefix = "x"; } ] "a.nix")).success'
same "prefix defaults empty"  "builtins.findFile [ { path = \"$W/dir1\"; } ] \"a.nix\""
same "not a list"             '(builtins.tryEval (builtins.findFile 1 "a.nix")).success'
# Not covered here, and named rather than left to look covered: both context
# branches -- a sought name carrying one, and an entry `path` carrying one --
# need a real store to make a context with, which this gate does not have.
# cppnix `forceStringNoCtx`s the name and `realiseContext`s the entry; the
# Rust side refuses the second by name and the first through the same
# forceStringNoCtx wording.

echo
echo "RESULT search-path-gate pairs=$pairs match=$match mismatch=$mismatch refused=$refused produced=$produced empty=$empty failed-alike=$failed_alike \
expected-pairs=$SEARCH_PATH_PAIRS expected-produced=$SEARCH_PATH_PRODUCED expected-refused=$SEARCH_PATH_REFUSED \
ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"
arms_require_rows "$pairs" "search path pairs"
if [ ${#failures[@]} -gt 0 ]; then
  printf 'FAILED: %s\n' "${failures[@]}"
  exit 1
fi
# `produced` used to be the only thing standing between an empty row and a
# green gate: a pair that agreed by printing nothing counted as `match` and
# merely failed to bump `produced`, so the floor was a ratchet rather than a
# verdict, and a second empty row could be traded against it. `arms_score`
# now calls that row EMPTY and fails it by name, which makes `produced`
# identical to `match` by construction -- kept because the ratchet still
# catches coverage leaving the gate a different way.
# Exact, from gate-ratchets.sh: every case here is a literal in this file.
ok=1
check_exact() { # NAME GOT WANT
  [ "$2" -eq "$3" ] && return 0
  echo "FAILED: $1=$2, gate-ratchets.sh says $3. Change the number there in the same commit that changes the case list; do not widen the comparison."
  ok=0
}
check_exact pairs    "$pairs"    "$SEARCH_PATH_PAIRS"
check_exact produced "$produced" "$SEARCH_PATH_PRODUCED"
# Zero today. A refusal here is not this gate's scope -- `<x>` is implemented --
# so one appearing means coverage left the gate, and `same()` counts it as
# neither match nor mismatch.
check_exact refused  "$refused"  "$SEARCH_PATH_REFUSED"
[ "$ok" -eq 1 ] || exit 1
echo "ALL SEARCH PATH CHECKS PASSED"
