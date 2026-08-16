#!/usr/bin/env bash
#
# Does a long evaluation die when the operator asks it to?
#
# **This gate asserts a deliberate divergence from cppnix, and records the
# other arm rather than requiring it.** cppnix checks no interrupt while
# evaluating -- `rg checkInterrupt src/libexpr` finds no site in eval.cc -- so
# a runaway evaluation there is unkillable by SIGTERM and is only noticed at
# the first checkpoint afterwards, which on this path is printing. The Rust VM
# is a poll machine and can check a flag cheaply, so under `eval-backend =
# rust` the same expression dies inside the bound below. An operator who can
# kill a runaway is better served than one who cannot. ENG-12533.
#
# The expression is deliberately pure: no `import`, no `readFile`, nothing
# that returns to the scheduler. A computation that suspends for IO was always
# interruptible between suspensions, so a gate built on one would pass with
# the check removed.
set -u

BUILD=${NIX_BUILD_DIR:-$HOME/incr-vm/nix/build}
NIXI=$BUILD/src/nix/nix-instantiate
[ -x "$NIXI" ] || { echo "no nix-instantiate at $NIXI (set NIX_BUILD_DIR)"; exit 2; }

# shellcheck source=./gate-ratchets.sh
. "$(cd "$(dirname "$0")" && pwd)/gate-ratchets.sh" || exit 2

# When SIGTERM is sent, and how long after that the hard SIGKILL follows. The
# hard kill exists only so a wedged process cannot hang the run; the bound
# that means something is SIGTERM_MAX_KILL_DELAY in gate-ratchets.sh.
AT=5
HARD=15
# What is actually measured: the gap between the signal and the process dying.
# This used to compare TOTAL elapsed against 15s, with the signal sent at 5s,
# so the arm could take ten seconds to notice SIGTERM and still pass. The
# check runs every 2048 poll iterations, which is microseconds, so the honest
# bound is a fraction of a second plus unwind time on a loaded box.
BOUND=$SIGTERM_MAX_KILL_DELAY

EXPR="builtins.foldl' (a: b: a + b) 0 (builtins.genList (x: builtins.foldl' (p: q: p + q) 0 (builtins.genList (y: builtins.stringLength (builtins.hashString \"sha512\" (toString (x * 1000 + y)))) 1000)) 45000)"

BASE="extra-experimental-features = rust-eval"
probe=$(NIX_CONFIG="$BASE
eval-backend = rust" "$NIXI" --eval --strict -E 1 2>&1)
[ "$probe" = "1" ] || { echo "sigterm-gate: rust arm cannot evaluate '1' ($probe); refusing to score"; exit 2; }

W=$(mktemp -d); trap 'rm -rf "$W"' EXIT

# The probe above proves the arm evaluates; this proves which backend did it.
# Derived from a per-backend evaluation count rather than echoed back from the
# setting (ENG-12542), so a build without the Rust evaluator cannot present
# itself as the rust arm and make this gate measure cppnix's interrupt
# behaviour -- which is the behaviour the header says diverges.
NIX_CONFIG="$BASE
eval-backend = rust" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/stats.json" \
  "$NIXI" --eval --strict -E 1 > /dev/null 2>&1
ev=$(python3 -c 'import json,sys
try:
    print(json.load(open(sys.argv[1])).get("evaluator", "<absent>"))
except OSError:
    print("<no stats file>")' "$W/stats.json")
[ "$ev" = "rust" ] || { echo "sigterm-gate: the rust arm reports evaluator='$ev'; this gate would be measuring the other backend"; exit 2; }

# Seconds with decimals: the thing being measured is a sub-second reaction,
# and whole-second `date +%s` arithmetic cannot see it at all.
#
# Values go in as argv, never interpolated into the program text. The first
# version of this built an f-string out of shell-quoted numbers, which is a
# syntax error on python 3.11, so every measurement came back as the empty
# string -- and the comparisons below, being `float('')`, raised, exited
# non-zero, and were read as "the bound was not exceeded". The gate printed
# `elapsed=s kill-delay=s` and PASSED. Caught by reading the output rather
# than the exit code.
now () { python3 -c 'import time; print("%.3f" % time.time())'; }
since () { python3 -c 'import sys; print("%.3f" % (float(sys.argv[2]) - float(sys.argv[1])))' "$1" "$2"; }

# A measurement that did not happen must not read as a measurement that
# passed. Everything below compares floats, and a non-numeric value makes
# every one of those comparisons quietly false.
numeric () { # NAME VALUE
  case $2 in
    ''|*[!0-9.-]*) echo "sigterm-gate: $1 came out as '$2', which is not a number; the run measured nothing"; exit 2 ;;
  esac
}

run () { # backend
  local backend=$1 start end
  start=$(now)
  NIX_CONFIG="$BASE
eval-backend = $backend" timeout -s TERM -k "$HARD" "$AT" \
    "$NIXI" --eval --strict -E "$EXPR" > "$W/$backend.out" 2> "$W/$backend.err"
  rc=$?
  end=$(now)
  elapsed=$(since "$start" "$end")
  numeric "elapsed ($backend arm)" "$elapsed"
  # How long after the signal it took to die. Negative means it finished on
  # its own before the signal was ever sent, which the caller must catch.
  delay=$(python3 -c 'import sys; print("%.3f" % (float(sys.argv[1]) - float(sys.argv[2])))' "$elapsed" "$AT")
  numeric "kill delay ($backend arm)" "$delay"
}

echo "== rust: must die within ${BOUND}s OF a SIGTERM sent at ${AT}s =="
run rust
rust_rc=$rc; rust_elapsed=$elapsed; rust_delay=$delay
echo "  rc=$rust_rc elapsed=${rust_elapsed}s kill-delay=${rust_delay}s err=[$(tr '\n' ' ' < "$W/rust.err" | head -c 200)]"

echo "== cpp: recorded, not required =="
run cpp
echo "  rc=$rc elapsed=${elapsed}s kill-delay=${delay}s err=[$(tr '\n' ' ' < "$W/cpp.err" | head -c 120)]"
cpp_note="rc=$rc elapsed=${elapsed}s"

fail=0
# The workload has to still be running when the signal arrives, or there is no
# interrupt to measure. Nothing checked this: as the VM gets faster, or on a
# quieter box, the expression finishes before AT and the run measures a
# completed evaluation instead. It would still have failed -- on the missing
# "interrupted by the user" line -- but reporting the wrong cause, and the
# repair would have been to weaken the wrong assertion.
if [ "$(python3 -c 'import sys; print(1 if float(sys.argv[1]) < float(sys.argv[2]) - 0.2 else 0)' "$rust_elapsed" "$AT")" = 1 ]; then
  echo "FAILED: the rust arm finished in ${rust_elapsed}s, before the SIGTERM at ${AT}s. Nothing was interrupted, so this run measured nothing; make EXPR bigger."
  fail=1
fi
# 137 is SIGKILL, which is the failure this gate exists to catch: the process
# ignored SIGTERM and the hard kill got it.
if [ "$rust_rc" = "137" ]; then
  echo "FAILED: the rust arm ignored SIGTERM and needed SIGKILL after ${HARD}s"
  fail=1
elif [ "$(python3 -c 'import sys; print(1 if float(sys.argv[1]) > float(sys.argv[2]) else 0)' "$rust_delay" "$BOUND")" = 1 ]; then
  echo "FAILED: the rust arm took ${rust_delay}s AFTER the signal to die, bound is ${BOUND}s (gate-ratchets.sh)"
  fail=1
fi
# The wording is cppnix's own, and checking it is what separates "died on the
# signal" from "happened to finish or crash".
if ! grep -q "interrupted by the user" "$W/rust.err"; then
  echo "FAILED: the rust arm did not report 'interrupted by the user'"
  fail=1
fi

echo
echo "RESULT sigterm-gate rust_rc=$rust_rc rust_elapsed=${rust_elapsed}s rust_kill_delay=${rust_delay}s \
bound=${BOUND}s signal_at=${AT}s cpp=[$cpp_note] \
ratchets-from=$GATE_RATCHETS_MEASURED_AT@$GATE_RATCHETS_MEASURED_ON"
[ "$fail" = 0 ] || exit 1
echo "SIGTERM CHECK PASSED"
