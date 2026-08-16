#!/usr/bin/env bash
#
# `flake-inputs-parity.sh`'s own guard. A guard you have not watched fail is
# not a guard, and that gate is almost entirely guards: its comparison rows
# came out 35 of 35 on the first run they were ever executed, which is the
# state in which a harness bug is least likely to be noticed and most likely
# to be believed.
#
# Each case copies the gate, replaces one or more literal spans, runs it, and
# requires it to FAIL with the message that names what was broken. The
# perturbations live here rather than behind a flag in the gate, so the gate
# carries no test scaffolding and cannot be run in a weakened mode by
# accident.
#
# Three refusals in this file, each of which caught a real mistake while it
# was being written:
#
#   - a span that is not in the gate is an error, not a skip. A perturbation
#     that quietly matched nothing tests nothing while reporting a pass.
#   - a perturbed gate that PASSES is a failure of the guard under test.
#   - a perturbed gate that fails with the WRONG message is also a failure.
#     The first run of this file had all eight cases in that state -- the
#     copies could not find the three helpers they source, so every one of
#     them died at exit 2 before reaching the guard it was written for. A
#     bare "did it fail" check would have called that a clean sweep.
#
# Slow: about 25s per case, four minutes in total, because every case is a
# full run of a gate that evaluates 70 flake installables. Not on the fast
# path; run it when you change the gate.
set -u
here=$(cd "$(dirname "$0")" && pwd)
GATE="$here/flake-inputs-parity.sh"
[ -f "$GATE" ] || { echo "no gate at $GATE"; exit 2; }

BUILD=${NIX_BUILD_DIR:-$PWD/build-rust}
[ -x "$BUILD/src/nix/nix" ] || { echo "no nix at $BUILD/src/nix/nix"; exit 2; }

W=$(mktemp -d) || exit 2
trap 'rm -rf "$W"' EXIT
fails=0; checked=0

# The perturbed copies run out of `$W`, and the gate resolves the three files
# it sources relative to its own directory.
for helper in gate-ratchets.sh error-class.sh compare-arms.sh arm-config.sh; do
  cp "$here/$helper" "$W/$helper" || exit 2
done

# Sanity: the unperturbed gate passes. Without this, every case below is
# satisfied by a gate failing for an unrelated reason, and the suite reports a
# clean sweep while measuring nothing.
echo "== baseline =="
if NIX_BUILD_DIR="$BUILD" bash "$GATE" > "$W/baseline.log" 2>&1; then
  echo "  ok       the unperturbed gate passes: $(grep -a '^RESULT' "$W/baseline.log")"
else
  echo "  WRONG    the unperturbed gate already fails, so no case below means anything:"
  tail -8 "$W/baseline.log" | sed 's/^/           /'
  exit 1
fi

break_case() { # NAME EXPECTED_SUBSTRING OLD NEW [OLD NEW]...
  local name=$1 want=$2
  shift 2
  local f="$W/gate-$name.sh"
  checked=$((checked + 1))
  # Variadic in pairs, because a perturbation is sometimes two edits that only
  # break anything together: adding an empty attribute to the fixtures does
  # nothing until the attribute list also names it, and the half-applied
  # version passed while reporting that it had perturbed the gate.
  if ! python3 -c '
import sys
src, dst = sys.argv[1:3]
pairs = sys.argv[3:]
if len(pairs) % 2:
    sys.exit("break_case wants OLD NEW in pairs")
s = open(src).read()
for old, new in zip(pairs[0::2], pairs[1::2]):
    if old not in s:
        sys.exit("the span to replace is not in the gate: " + old[:70])
    s = s.replace(old, new, 1)
open(dst, "w").write(s)
' "$GATE" "$f" "$@"; then
    echo "  WRONG    $name: could not perturb the gate"
    fails=$((fails + 1)); return
  fi
  local rc
  NIX_BUILD_DIR="$BUILD" bash "$f" > "$W/$name.log" 2>&1
  rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "  WRONG    $name: the perturbed gate PASSED"
    fails=$((fails + 1))
  elif LC_ALL=C grep -aqF "$want" "$W/$name.log"; then
    printf '  ok       %-16s exit %s, refused with: %s\n' "$name" "$rc" "$want"
  else
    echo "  WRONG    $name: failed (exit $rc) but not with '$want':"
    LC_ALL=C grep -aE 'flake-inputs-parity|coverage |provenance |shape |compare-arms' "$W/$name.log" \
      | tail -4 | sed 's/^/           /'
    fails=$((fails + 1))
  fi
}

# Every argument below is a literal span of `flake-inputs-parity.sh`'s source,
# handed to `str.replace`, so shellcheck's two complaints about it are both
# reports that it is doing what it is for: SC2016 sees `$RUST` in single
# quotes and says it will not expand (correct -- the gate expands it, this
# file must not), and SC1003 sees a span ending in the gate's own line
# continuation and offers to escape a quote that is really the string's
# closing one.
#
# Scoped to a function holding nothing but the perturbations rather than put
# at the top of the file, so an accidentally-unexpanded variable in the
# scaffolding above or below is still reported. Nothing inside is indented:
# each payload is a literal span of another file, and two leading spaces on a
# continuation line would stop it matching.
# shellcheck disable=SC2016,SC1003
perturbations() {
echo "== perturbations =="

# 1. The pre-lock is what puts a node on the lazy branch of `computeLocks` and
# so through `fetchTreeFinal`. Delete the lock and the coverage read becomes
# the run that creates it, which covers every node.
#
# Deleting it, and NOT re-creating it with `flake lock --recreate-lock-file`:
# the first version of this case did the latter and the gate PASSED, because
# the recreate runs in its own process and the next process still finds an
# up-to-date lock and still takes the lazy branch. What puts a run on the
# eager path is being the run that creates the lock, not the lock being young.
break_case relocked "coverage abspath   all-overridden" \
'  NIX_CONFIG="$RUST" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/fstats-$label.json" \' \
'  rm -f "$dir/flake.lock"
  NIX_CONFIG="$RUST" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/fstats-$label.json" \'

# 2. The whole gate is vacuous if the flake path falls back to cppnix, because
# both arms then agree perfectly. Take the provenance reading on the cpp arm
# and the counters must give it away.
break_case cpp-fallback "did not evaluate on the VM" \
'  NIX_CONFIG="$RUST" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/fstats-$label.json" \' \
'  NIX_CONFIG="$CPP" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/fstats-$label.json" \'

# 3. A row where both arms succeed and print nothing is the four-times bug
# `compare-arms.sh` exists for. Two edits: the fixtures grow an attribute that
# is the empty string, and the attribute list names it.
break_case empty-row "agreement about nothing" \
'    marker = $5;' \
'    marker = $5;
    emptyAttr = "";' \
'  "marker|raw"' \
'  "emptyAttr|raw"
  "marker|raw"'

# 4. A printed drvPath with no file at it is ENG-12799. Look for the `.drv`
# where it cannot be, and the tier-1 half must refuse rather than score the
# printed strings and stop.
break_case drv-missing "left no file at it" \
'      if [ -n "$rpath" ] && [ -f "$rpath" ]; then' \
'      if [ -n "$rpath" ] && [ -f "$rpath.notthere" ]; then'

# 5. A fixture that lost the shape it is named for still agrees on both arms
# and still passes every comparison, silently covering nothing.
break_case shape-lost "isRelative branch never runs" \
"  '  inputs.dep.url = \"path:./dep\";' \\" \
'  "  inputs.dep.url = \"path:$FIX/dep\";" \'

# 6. The comparison itself. Send the rust arm at a different fixture; the row
# scorer must call that a divergence rather than shrug.
break_case real-divergence "row(s) diverged" \
'  case $1 in cpp) cfg=$CPP ;; *) cfg=$RUST ;; esac
  case $3 in' \
'  case $1 in cpp) cfg=$CPP ;; *) cfg=$RUST ;; esac
  [ "$1" != cpp ] && set -- "$1" "${2/r-abspath/r-git}" "$3" "$4" "$5"
  case $3 in'

# 7. An arm that re-locks mid-run means the two arms read different lock
# files. Simulated by moving a lock's bytes after its hash is taken: a
# trailing newline changes the file and not the JSON, so nothing else notices.
break_case lock-drift "flake.lock changed during the run" \
'declare -a ATTRS=(' \
'printf "\n" >> "$FIX/r-abspath/flake.lock"
declare -a ATTRS=('

# 8. The oracle, and this case is the reason it exists. Both getFlake arms are
# pointed at a DIFFERENT fixture, identically, so the cross-arm comparison
# agrees perfectly -- cpp and rust both evaluate the git fixture and both
# produce the same bytes. Only the check against the command line notices that
# `getFlake "path:.../r-abspath"` stopped answering for `r-abspath`.
#
# That is the shape a second, subtly different overrides document would take:
# both arms wrong together, store paths moving in step, every cross-arm row
# green. Without this case the oracle is a line of code nobody has seen work.
break_case oracle-blind "getFlake and the command line disagree" \
'  expr="(builtins.getFlake \"path:$2\").$3"' \
'  expr="(builtins.getFlake \"path:${2/r-abspath/r-git}\").$3"'

# 9. A run that compared nothing. Empty the attribute list -- rather than the
# fixture list, which the coverage phase would refuse first -- and the
# scorer's zero-row refusal has to fire before any count is believed.
break_case no-rows "measured nothing" \
'  "marker|raw"
  "depOutPath|raw"
  "meta|json"
  "packages.$SYSTEM.fixture.drvPath|drv"
  "packages.$SYSTEM.fixture.outPath|raw"' \
'  # every attribute removed by the selftest'

}
perturbations

echo
echo "RESULT flake-inputs-parity-selftest checked=$checked failed=$fails"
[ "$fails" -eq 0 ] || exit 1
echo "flake-inputs-parity-selftest: OK"
