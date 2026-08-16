#!/usr/bin/env bash
# Does import-from-derivation overlap on the rust backend? (ENG-13150)
#
# One evaluation, N derivations that each burn a fixed amount of CPU, every
# one imported -- so every one is an IFD build the evaluator has to wait for.
# Before ENG-13150 the scheduler waited for each in turn and the wall clock
# was ~N times one build; with the threaded host path the builds are in
# flight together and the wall clock is ~one build plus overhead.
#
# The builds burn CPU in a pure-shell loop rather than calling `sleep`,
# because the sandbox gives a builder no PATH and no coreutils -- an earlier
# draft's `sleep 5; echo ...` "passed" in 1.4s with `sleep: command not
# found` on stderr, which is precisely the silent nothing-measured failure a
# timing gate must refuse to emit. A loop of shell builtins runs everywhere
# a shell runs, and its duration is CALIBRATED rather than assumed: phase A
# builds one derivation alone and times it, phase B builds N together, and
# the verdict is relative:
#
#   t1 >= MIN_BUILD        the calibration build took long enough to time
#                          (auto-rescaled once if the machine is too fast)
#   tN <  2 * t1           N builds in not much more than one build's time;
#                          serial would be ~N*t1, so anything under 2*t1
#                          proves the builds were genuinely in flight
#                          together. N cores must be free for this to hold,
#                          which is what "run it on an idle dev node" means.
#
# It also asserts what ENG-13151 demands of the same machinery: a second run
# of the same evaluation prints byte-identical stdout. The second run's
# builds are already valid, so it exercises the same begin/collect path with
# instant answers -- the delivery order must be the program's, not the
# builders', and a byte difference here is the race the scheduler's
# token-mint-order rule exists to prevent.
#
# Needs a built nix with the Rust evaluator linked in (see drv-parity.sh for
# the meson invocation) and a store that can BUILD -- the same requirement as
# drv-parity's nix build arm. Point NIX_BUILD_DIR at the build tree; the
# default is ./build-rust.
set -u

BUILD=${NIX_BUILD_DIR:-$PWD/build-rust}
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || { echo "no nix-instantiate at $NIXI"; exit 2; }

here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=./arm-config.sh
. "$here/arm-config.sh" || exit 2
arm_pin_environment

# How many concurrent builds, how many loop iterations each burns, and the
# shortest calibration build this script will accept a verdict from.
N=${IFD_OVERLAP_N:-3}
LOOP=${IFD_OVERLAP_LOOP:-2000000}
MIN_BUILD=${IFD_OVERLAP_MIN_BUILD:-3}

RUST="extra-experimental-features = rust-eval nix-command
$(arm_base_config)
eval-backend = rust"

W=$(mktemp -d); trap 'rm -rf "$W"' EXIT

# Capability probe, as every gate has: a binary without the Rust evaluator
# reports the setting and serves cpp, and this script would then time the
# blocking backend and call whatever it saw a verdict about the threaded one.
NIX_CONFIG="$RUST" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats.json" \
  "$NIXI" --eval --strict -E 1 > /dev/null 2>&1
ev=$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))
except OSError:
    print("<no stats file>")' "$W/stats.json")
[ "$ev" = rust ] || {
  echo "ifd-overlap: asked for the rust evaluator, NIX_SHOW_STATS reports '$ev'; timing the wrong backend proves nothing"
  exit 2
}

# The salt is what makes every run build rather than substitute: it is in
# every derivation's name, so the outputs cannot already be in the store.
SALT="$(date +%s)-$$"

# An expression evaluating to COUNT imported build results, each burning
# LOOP shell-arithmetic iterations. `--read-write-mode`, because IFD writes
# the .drv and builds it, and nix-instantiate --eval is read-only by default.
emit_expr() { # $1 = count, $2 = phase tag
  cat <<EOF
let
  mk = i: import (derivation {
    name = "ifd-overlap-$2-\${i}-${SALT}";
    system = builtins.currentSystem;
    builder = "/bin/sh";
    args = [ "-c" "n=0; while [ \\\$n -lt ${LOOP} ]; do n=\\\$((n+1)); done; echo '\"built-\${i}\"' > \\\$out" ];
  });
in builtins.genList (n: mk (builtins.toString n)) $1
EOF
}

evaluate() { # $1 = expression file, $2 = stdout file, $3 = stderr file
  NIX_CONFIG="$RUST" "$NIXI" --eval --strict --read-write-mode "$1" > "$2" 2> "$3"
}

timed() { # $1..$3 as evaluate; echoes whole seconds
  local start
  start=$(date +%s)
  evaluate "$1" "$2" "$3" || return 1
  echo $(( $(date +%s) - start ))
}

# -- phase A: calibrate one build -------------------------------------------
calibrate() {
  emit_expr 1 calibrate > "$W/one.nix"
  t1=$(timed "$W/one.nix" "$W/one.out" "$W/one.err") || {
    echo "ifd-overlap: the calibration build failed; nothing to time:"
    cat "$W/one.err"
    echo "RESULT ifd-overlap FAIL n=$N loop=$LOOP (calibration eval failed)"
    exit 1
  }
}
calibrate
if [ "$t1" -lt "$MIN_BUILD" ]; then
  # Too fast to time against. Scale the loop once, deterministically, and
  # recalibrate; a machine that still finishes under MIN_BUILD gets a FAIL
  # that says so rather than a verdict about noise.
  LOOP=$(( LOOP * (MIN_BUILD + 1) / (t1 + 1) + LOOP ))
  SALT="$SALT-rescaled"
  echo "calibration build finished in ${t1}s (< ${MIN_BUILD}s); rescaling loop to $LOOP"
  calibrate
fi
echo "calibration: one build takes ${t1}s (loop=$LOOP)"
if [ "$t1" -lt "$MIN_BUILD" ]; then
  echo "RESULT ifd-overlap FAIL n=$N loop=$LOOP t1=${t1}s (too fast to time; raise IFD_OVERLAP_LOOP)"
  exit 1
fi

# -- phase B: N builds through one evaluation --------------------------------
emit_expr "$N" overlap > "$W/many.nix"
tN=$(timed "$W/many.nix" "$W/run1.out" "$W/run1.err") || {
  echo "ifd-overlap: the overlap evaluation failed:"
  cat "$W/run1.err"
  echo "RESULT ifd-overlap FAIL n=$N loop=$LOOP t1=${t1}s (overlap eval failed)"
  exit 1
}

# Byte-identical on a rerun (ENG-13151). The rerun's builds are valid, so it
# answers through the same begin/collect path without waiting.
evaluate "$W/many.nix" "$W/run2.out" "$W/run2.err" || {
  echo "ifd-overlap: the rerun failed where the first run succeeded:"
  cat "$W/run2.err"
  echo "RESULT ifd-overlap FAIL n=$N loop=$LOOP t1=${t1}s tN=${tN}s (rerun failed)"
  exit 1
}

bound=$(( 2 * t1 ))
serial=$(( N * t1 ))
deterministic=yes
cmp -s "$W/run1.out" "$W/run2.out" || deterministic=no

verdict=pass
[ "$tN" -lt "$bound" ] || verdict=FAIL   # no overlap
[ "$deterministic" = yes ] || verdict=FAIL

echo "first run:  $(tr -d '\n' < "$W/run1.out")"
echo "wall ${tN}s for $N builds of ~${t1}s each (serial would be ~${serial}s; overlap bound <${bound}s)"
echo "RESULT ifd-overlap $verdict n=$N loop=$LOOP t1=${t1}s tN=${tN}s serial=${serial}s bound=${bound}s deterministic=$deterministic"
[ "$verdict" = pass ]
