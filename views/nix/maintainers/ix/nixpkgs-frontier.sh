#!/usr/bin/env bash
#
# How far into nixpkgs does the Rust backend get, and what stops it?
#
# Not a gate about REFUSALS: most of these are expected to refuse today, and
# the output is the sequencing information -- which rung the next blocker
# belongs to -- that corpus file counts do not give. The ladder's "Where the
# frontier actually is" section is this script's output.
#
# It is a gate about DIFFER, which this header always said was a bug and
# nothing acted on: the script reported one and exited 0. A rule that is
# declared and unenforced is not a rule, so a DIFFER now exits non-zero, the
# row count is checked against gate-ratchets.sh, and the frontier may not
# retreat below the agree count recorded there.
#
# Each line runs one expression twice through one binary, cpp then rust, and
# reports AGREE, DIFFER, TIMEOUT, EMPTY or REFUSED with the refusal's own
# words. A REFUSED line names the next thing to implement.
#
#   NIXPKGS=/nix/store/...-source NIX_BUILD_DIR=$PWD/build-rust ./nixpkgs-frontier.sh
#
# This needs a built nix-instantiate, so it needs a dev node. If the question is
# "what stops the evaluator here?" -- which is most of them -- ask
# `cargo run --release --example nixpkgs-probe` in rust/ first: same twelve
# expressions, 3.5s on a laptop, any machine, no C++ build. It is single-arm and skips the bridge
# entirely, so it bisects and this decides. See maintainers/ix/testing.md.
set -u

BUILD=${NIX_BUILD_DIR:-$HOME/incr-vm/nix/build}
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || { echo "no nix-instantiate at $NIXI (set NIX_BUILD_DIR)"; exit 2; }
# shellcheck source=./gate-ratchets.sh
. "$(cd "$(dirname "$0")" && pwd)/gate-ratchets.sh" || exit 2
# shellcheck source=./compare-arms.sh
. "$(cd "$(dirname "$0")" && pwd)/compare-arms.sh" || exit 2
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
W_ERR=$W/nixpkgs-resolve.err

# The tree this repo pins, read out of flake.lock, and NOT the flake registry.
#
# The registry floats. `(builtins.getFlake "nixpkgs").outPath` resolves to
# whatever the machine's registry points at today, so the checked-in
# `NIXPKGS_FRONTIER_MIN_AGREE` was a floor measured against a moving target:
# a registry bump could take the gate red, or green, with no commit in this
# repo and nothing to attribute it to. Measured both ways on one binary
# (ENG-12855): the floating 26.11pre tree scored 12/12 while the pinned 25.11
# tree scored 6/12, so the two are not interchangeable and the floor only
# means something beside the tree it was measured on.
#
# Errors are kept rather than dropped on the floor: this used to end in
# `2>/dev/null`, so a missing registry, a network failure and a typo all
# arrived as the same empty string and the same "no nixpkgs source".
NIXPKGS=${NIXPKGS:-}
LOCK=$(cd "$(dirname "$0")/../.." && pwd)/flake.lock
if [ -z "$NIXPKGS" ]; then
  [ -f "$LOCK" ] || { echo "nixpkgs-frontier: no flake.lock at $LOCK, and NIXPKGS is unset"; exit 2; }
  lock_type=$(jq -r '.nodes.nixpkgs.locked.type // "?"' "$LOCK")
  # Only the shape this repo actually pins is handled. A `github` or `path`
  # node would need a different fetch, and guessing one produces a tree that
  # is not what the lock names -- which is the whole failure this replaces.
  [ "$lock_type" = tarball ] || {
    echo "nixpkgs-frontier: flake.lock pins nixpkgs as '$lock_type', and this script"
    echo "only knows how to fetch a 'tarball' node. Set NIXPKGS explicitly, or teach"
    echo "it that shape."
    exit 2
  }
  lock_url=$(jq -r '.nodes.nixpkgs.locked.url' "$LOCK")
  lock_hash=$(jq -r '.nodes.nixpkgs.locked.narHash' "$LOCK")
  NIXPKGS=$(nix eval --raw --impure --expr \
    "builtins.fetchTarball { url = \"$lock_url\"; sha256 = \"$lock_hash\"; }" 2>"$W_ERR") || {
    echo "nixpkgs-frontier: could not fetch the pinned nixpkgs ($lock_url):"
    cat "$W_ERR"
    exit 2
  }
  echo "nixpkgs-frontier: using the tree flake.lock pins ($lock_hash)"
fi
[ -d "$NIXPKGS" ] || { echo "no nixpkgs source at '$NIXPKGS' (set NIXPKGS)"; exit 2; }

BASE="extra-experimental-features = rust-eval"
CPP="$BASE
eval-backend = cpp"
RUST="$BASE
eval-backend = rust"

probe=$(NIX_CONFIG="$RUST" "$NIXI" --eval --strict -E 1 2>&1)
[ "$probe" = "1" ] || { echo "nixpkgs-frontier: rust arm cannot evaluate '1' ($probe)"; exit 2; }

echo "nixpkgs=$NIXPKGS"
echo "bin=$NIXI sha256=$(sha256sum "$NIXI" | cut -d' ' -f1)"
echo

# `timeout` reports 124 when it kills the command. Both arms hitting that
# produced two empty stdouts and two equal exit codes, which `cmp -s` called
# AGREE -- the loudest possible pass for the case where neither arm answered
# at all. It is its own verdict now, and it fails the run.
TIMEOUT_RC=124
n=0; agree=0; differ=0; refused=0; timedout=0; empty=0
row () {
  local label=$1 expr=$2
  n=$((n + 1))
  NIX_CONFIG="$CPP" timeout "$NIXPKGS_FRONTIER_ROW_TIMEOUT" "$NIXI" --eval --strict -I "nixpkgs=$NIXPKGS" -E "$expr" \
    > "$W/cpp.out" 2> "$W/cpp.err"; local rc_cpp=$?
  NIX_CONFIG="$RUST" timeout "$NIXPKGS_FRONTIER_ROW_TIMEOUT" "$NIXI" --eval --strict -I "nixpkgs=$NIXPKGS" -E "$expr" \
    > "$W/rust.out" 2> "$W/rust.err"; local rc_rust=$?
  local verdict detail
  if [ $rc_cpp -eq $TIMEOUT_RC ] || [ $rc_rust -eq $TIMEOUT_RC ]; then
    verdict=TIMEOUT; timedout=$((timedout + 1))
    detail="cpp rc=$rc_cpp rust rc=$rc_rust after ${NIXPKGS_FRONTIER_ROW_TIMEOUT}s; this row measured nothing"
  elif [ $rc_rust -ne 0 ] && grep -q "rust-eval unimplemented" "$W/rust.err"; then
    verdict=REFUSED; refused=$((refused + 1))
    detail=$(grep -o 'rust-eval unimplemented: .*' "$W/rust.err" | head -1 | cut -c1-140)
  else
    # Agreeing about a value and agreeing about having produced no value are
    # different claims, and only the first is what this row set out to ask.
    # The split lives in `compare-arms.sh` now rather than here, because this
    # gate was the third place it got written and the second place it got
    # written wrong.
    arms_score "$W/cpp.out" "$rc_cpp" "$W/rust.out" "$rc_rust"
    case $ARMS_VERDICT in
    match)
      verdict=AGREE; agree=$((agree + 1))
      detail=$(head -c 90 "$W/cpp.out")
      ;;
    # Both arms refused it identically, or both succeeded silently. Neither is
    # agreement about a value; every row in this gate's list is meant to
    # produce one, so both are counted apart and both fail the run below.
    empty|fail-both)
      verdict=EMPTY; empty=$((empty + 1))
      detail="both arms exited $rc_cpp and neither printed anything; cpp err=[$(head -c 80 "$W/cpp.err" | tr '\n' ' ')]"
      ;;
    *)
      verdict=DIFFER; differ=$((differ + 1))
      detail="cpp rc=$rc_cpp [$(head -c 60 "$W/cpp.out")$(head -c 60 "$W/cpp.err" | tr '\n' ' ')] rust rc=$rc_rust [$(head -c 60 "$W/rust.out")$(head -c 90 "$W/rust.err" | tr '\n' ' ')]"
      ;;
    esac
  fi
  printf '%-2s %-34s %-8s %s\n' "$n" "$label" "$verdict" "$detail"
}

row "the lookup itself"      'builtins.typeOf <nixpkgs>'
row "lib alone"              '(import <nixpkgs/lib>).version'
row "lib attr count"         'builtins.length (builtins.attrNames (import <nixpkgs/lib>))'
row "lib.strings"            '(import <nixpkgs/lib>).strings.toUpper "abc"'
row "the top-level function" 'builtins.typeOf (import <nixpkgs>)'
row "the package set"        'builtins.typeOf (import <nixpkgs> {})'
row "one package name"       '(import <nixpkgs> {}).hello.name'
row "one package outPath"    '(import <nixpkgs> {}).hello.outPath'
row "stdenv"                 '(import <nixpkgs> {}).stdenv.name'
row "currentSystem"          'builtins.currentSystem'
row "a small package set"    'builtins.typeOf (import <nixpkgs> { system = "x86_64-linux"; })'
row "package set attr count" 'builtins.length (builtins.attrNames (import <nixpkgs> { system = "x86_64-linux"; }))'

echo
echo "RESULT nixpkgs-frontier rows=$n agree=$agree differ=$differ refused=$refused \
timeout=$timedout empty=$empty expected-rows=$NIXPKGS_FRONTIER_ROWS min-agree=$NIXPKGS_FRONTIER_MIN_AGREE \
ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"

ok=1
# `differ=0` is the invariant, enforced here rather than left to a reader: a
# REFUSED row is a gap this backend admits to, a DIFFER row is the two
# evaluators disagreeing about a real expression. Refusals are expected and
# do not fail this; a single DIFFER does.
#
# 4e4f6c7ff added exactly this check, independently and at the same time as
# this branch. Its wording is kept; the rest below is what this branch adds.
if [ "$differ" -ne 0 ]; then
  echo "FAILED: $differ row(s) differ. A refusal is a gap; a difference is a bug."
  ok=0
fi
# A row that neither arm finished, or that both arms failed with no value, is
# not agreement -- and scored as AGREE until now, because two killed processes
# have equal exit codes and two empty stdouts.
if [ "$timedout" -ne 0 ] || [ "$empty" -ne 0 ]; then
  echo "FAILED: $timedout row(s) timed out and $empty produced nothing on either arm. Those rows compared nothing; before this check they scored AGREE."
  ok=0
fi
# The row list is checked in above, so its length is a fact, not a measurement.
arms_require_rows "$n" "frontier rows"
if [ "$n" -ne "$NIXPKGS_FRONTIER_ROWS" ]; then
  echo "FAILED: ran $n rows, gate-ratchets.sh says $NIXPKGS_FRONTIER_ROWS. Update it in the same commit that changes the row list."
  ok=0
fi
# The frontier may advance and may not retreat. Without this, a backend that
# started refusing every row would report differ=0 and read as healthy.
if [ "$agree" -lt "$NIXPKGS_FRONTIER_MIN_AGREE" ]; then
  echo "FAILED: $agree rows agree, the checked-in floor is $NIXPKGS_FRONTIER_MIN_AGREE. The frontier went backwards."
  ok=0
fi
[ "$ok" -eq 1 ]
