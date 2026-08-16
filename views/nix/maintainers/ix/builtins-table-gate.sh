#!/usr/bin/env bash
#
# Do the two backends advertise the same `builtins`?
#
# `builtins ? name` is the standard capability test in nixpkgs and in our own
# Nix, and it exists so that code can take a working path when a builtin is
# absent. A backend that answers `true` for something it then refuses inverts
# that: the defensive branch is the one that cannot run. Measured at
# bbb0abfacc1c, cpp advertised 118 names and rust 126, and
# `if builtins ? fetchClosure then <fast> else <fallback>` took the fast path
# under rust and the fallback under cpp. ENG-12717.
#
# The comparison is the whole name list, not a count: two sets can be the same
# size and differ. Run under several configurations, because which names
# cppnix registers depends on the experimental feature set, on
# allow-unsafe-native-code-during-evaluation, and -- for `wasm` -- on whether
# the build has wasmtime at all. The last one is why the Rust side takes
# cppnix's own list rather than re-deriving the rules.
#
# Needs a built nix with the Rust evaluator linked in. Point it at one with
# NIX_BUILD_DIR; the default matches the other gates.
set -u

BUILD=${NIX_BUILD_DIR:-$HOME/incr-vm/nix/build}
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || { echo "no nix-instantiate at $NIXI (set NIX_BUILD_DIR)"; exit 2; }
# shellcheck source=./gate-ratchets.sh
. "$(cd "$(dirname "$0")" && pwd)/gate-ratchets.sh" || exit 2
# shellcheck source=./arm-config.sh
. "$(cd "$(dirname "$0")" && pwd)/arm-config.sh" || exit 2
# One owner of the gates' nix configuration, before anything reads the
# environment: an ambient `lint-url-literals = fatal` otherwise makes every
# rust arm refuse and every row score `unimplemented` (ENG-12996).
arm_pin_environment


W=$(mktemp -d); trap 'rm -rf "$W"' EXIT

# lint-url-literals=warn because this backend refuses `fatal` by name, which
# would make every rust arm below a refusal rather than an answer.
cfg() { printf 'experimental-features = %s\n%s\n%s\neval-backend = %s\n' "$1" "$(arm_base_config)" "$3" "$2"; }

# A binary built without the Rust evaluator reports `eval-backend = rust` and
# runs the cpp one, which would make every case here a pair of identical cpp
# runs scoring a clean sheet. Probe by evaluating, then by asking which
# backend actually served it.
for arm in cpp rust; do
  out=$(NIX_CONFIG="$(cfg "nix-command flakes rust-eval" "$arm" "")" \
    NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats-$arm.json" \
    "$NIXI" --eval --strict -E 1 2>&1)
  [ "$out" = "1" ] || { echo "builtins-table-gate: the $arm arm cannot evaluate '1' ($out); refusing to score"; exit 2; }
  ev=$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))
except OSError:
    print("<no stats file>")' "$W/stats-$arm.json")
  [ "$ev" = "$arm" ] || {
    echo "builtins-table-gate: the $arm arm reports evaluator='$ev'; both arms would be the same backend and every comparison below would be vacuous"
    exit 2
  }
done

rows=0; agree=0; differ=0
failures=()

compare() { # LABEL FEATURES EXTRA-CONFIG
  local label=$1 features=$2 extra=${3:-}
  rows=$((rows + 1))
  NIX_CONFIG="$(cfg "$features" cpp  "$extra")" "$NIXI" --eval --strict --json \
    -E 'builtins.attrNames builtins' > "$W/cpp.json" 2> "$W/cpp.err"
  NIX_CONFIG="$(cfg "$features" rust "$extra")" "$NIXI" --eval --strict --json \
    -E 'builtins.attrNames builtins' > "$W/rust.json" 2> "$W/rust.err"

  # An empty or unparseable list on either side is a broken run, not agreement
  # between two empty sets.
  local n_cpp n_rust
  n_cpp=$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))))' "$W/cpp.json" 2>/dev/null || echo 0)
  n_rust=$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))))' "$W/rust.json" 2>/dev/null || echo 0)
  if [ "$n_cpp" -lt 100 ] || [ "$n_rust" -lt 100 ]; then
    differ=$((differ + 1)); failures+=("$label")
    echo "  BROKEN   $label -- cpp=$n_cpp rust=$n_rust names; a list this short is a failed run, not a small builtins set"
    echo "    cpp  err=[$(head -c 200 "$W/cpp.err")]"
    echo "    rust err=[$(head -c 200 "$W/rust.err")]"
    return
  fi
  if cmp -s "$W/cpp.json" "$W/rust.json"; then
    agree=$((agree + 1))
    echo "  ok       $label -- both advertise $n_cpp names"
    return
  fi
  differ=$((differ + 1)); failures+=("$label")
  echo "  DIFFER   $label -- cpp=$n_cpp rust=$n_rust"
  python3 - "$W/cpp.json" "$W/rust.json" <<'PY'
import json, sys
cpp = set(json.load(open(sys.argv[1])))
rust = set(json.load(open(sys.argv[2])))
print("    rust only:", " ".join(sorted(rust - cpp)) or "(none)")
print("    cpp only: ", " ".join(sorted(cpp - rust)) or "(none)")
PY
}

echo "== the whole name list, per configuration =="
compare "default (nix-command flakes)"  "nix-command flakes rust-eval"
compare "no flakes, so no fetch-tree"   "nix-command rust-eval"
compare "fetch-closure"                 "nix-command flakes rust-eval fetch-closure"
compare "dynamic-derivations"           "nix-command flakes rust-eval dynamic-derivations ca-derivations"
compare "parallel-eval"                 "nix-command flakes rust-eval parallel-eval"
compare "wasm-builtin"                  "nix-command flakes rust-eval wasm-builtin"
compare "every gate at once"            "nix-command flakes rust-eval fetch-closure dynamic-derivations ca-derivations parallel-eval wasm-builtin"
compare "native code"                   "nix-command flakes rust-eval" "allow-unsafe-native-code-during-evaluation = true"

echo
echo "== the capability test itself, which is what the divergence broke =="
# One expression per gated name, in the shape real code writes: the answer is
# a string naming the branch taken, so a divergence shows up as two different
# working answers rather than as an error on one side.
capability() { # LABEL FEATURES NAME
  local label=$1 features=$2 name=$3
  rows=$((rows + 1))
  local expr="if builtins ? $name then \"fast-path\" else \"fallback\""
  local c r
  c=$(NIX_CONFIG="$(cfg "$features" cpp  "")" "$NIXI" --eval --strict -E "$expr" 2>&1)
  r=$(NIX_CONFIG="$(cfg "$features" rust "")" "$NIXI" --eval --strict -E "$expr" 2>&1)
  case "$c" in '"fast-path"'|'"fallback"') ;; *)
    differ=$((differ + 1)); failures+=("$label")
    echo "  BROKEN   $label -- the cpp arm answered [$c], which is neither branch"
    return ;;
  esac
  if [ "$c" = "$r" ]; then
    agree=$((agree + 1)); echo "  ok       $label -- both take $c"
  else
    differ=$((differ + 1)); failures+=("$label")
    echo "  DIFFER   $label -- cpp=$c rust=$r"
  fi
}
for name in fetchClosure outputOf parallel wasm exec importNative fetchFinalTree recordedTreeAttr; do
  capability "off:  builtins ? $name" "nix-command flakes rust-eval" "$name"
done
capability "on:   builtins ? fetchClosure" "nix-command flakes rust-eval fetch-closure" fetchClosure
capability "on:   builtins ? outputOf"     "nix-command flakes rust-eval dynamic-derivations ca-derivations" outputOf

echo
echo "RESULT builtins-table-gate rows=$rows agree=$agree differ=$differ \
expected-rows=$BUILTINS_TABLE_ROWS \
ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"
if [ ${#failures[@]} -gt 0 ]; then
  printf 'FAILED: %s\n' "${failures[@]}"
  exit 1
fi
# Every row is a literal above, so the count is exact rather than a floor: a
# row that stopped running would otherwise leave the gate green with nothing
# compared. Checked after the failure list so a real divergence is reported
# as itself rather than as a miscount.
if [ "$rows" -ne "$BUILTINS_TABLE_ROWS" ]; then
  echo "FAILED: rows=$rows, gate-ratchets.sh says $BUILTINS_TABLE_ROWS. Change the number there in the same commit that changes the case list; do not widen the comparison."
  exit 1
fi
if [ "$agree" -ne "$rows" ]; then
  echo "FAILED: agree=$agree of rows=$rows. Every row must agree; the gate exists because a row that quietly stopped agreeing is ENG-12717."
  exit 1
fi
echo "ALL BUILTINS TABLE CHECKS PASSED"
