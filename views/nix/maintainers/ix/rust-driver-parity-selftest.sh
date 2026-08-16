#!/usr/bin/env bash
#
# `rust-driver-parity.sh`'s own guard.
#
# A guard you have not watched fail is not a guard, and that gate came out 21
# of 21 on the first run it was ever executed -- the state in which a harness
# bug is least likely to be noticed and most likely to be believed. The break
# tests that found the real holes in it were run by hand, and a hand-run break
# test is not re-run by anyone. This file is those tests, kept.
#
# It has already earned its place. The by-hand version of case `system` below
# PASSED against the gate as first written, because every corpus case spelled
# `system` as a literal and so nothing ever read `builtins.currentSystem` --
# the gate went to the trouble of passing `--system` to the driver and then
# never consulted it. That is what added two cases to the corpus.
#
# ## How a case breaks the driver without rebuilding it
#
# `rust-driver-parity.sh` takes the driver as its second argument, so a case
# is a small wrapper that delegates to the real binary and corrupts exactly
# one thing on the way past. No `cargo build` per case, and -- more
# importantly -- the gate under test is the real file with no test scaffolding
# in it, so it cannot be run in a weakened mode by accident.
#
# The one case that perturbs the gate's own source instead is `emptycorpus`,
# because what it asserts is about the gate's reporting rather than about any
# driver behaviour.
#
# ## Three ways a case is WRONG, not just one
#
#   - a perturbed run that PASSES is a failure of the gate under test
#   - a perturbed run that fails with the WRONG message is also a failure: a
#     bare "did it fail" check is satisfied by the gate dying at its probe
#   - the unperturbed gate must pass first, or every case below is satisfied
#     by something unrelated and the suite reports a clean sweep having
#     measured nothing
#
# Usage: rust-driver-parity-selftest.sh BUILDDIR/src/nix [DRIVER]
#
# Slow: each case is a full 21-case run of the gate, so budget a couple of
# minutes per case. Run it when you change the gate, not on the fast path.

set -u
set -o pipefail

BIN=${1:-}
[ -n "$BIN" ] || { echo "usage: rust-driver-parity-selftest.sh BUILDDIR/src/nix [DRIVER]"; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
GATE="$here/rust-driver-parity.sh"
[ -f "$GATE" ] || { echo "no gate at $GATE"; exit 2; }
REAL=${2:-$repo/rust/target/release/nix-eval-driver}
[ -x "$REAL" ] || { echo "no nix-eval-driver at $REAL"; exit 2; }

W=$(cd "$(mktemp -d)" && pwd -P) || exit 2
# Invoked from the EXIT trap, which shellcheck cannot see (SC2329).
# shellcheck disable=SC2329
scrub() {
  for victim in "$@"; do
    [ -e "$victim" ] || continue
    chmod -R u+w "$victim" 2>/dev/null
    rm -rf "$victim"
  done
}
trap 'scrub "$W"' EXIT
fails=0; checked=0

# The perturbed copy runs out of `$W` and the gate resolves the files it
# sources relative to its OWN directory, so without these it dies at
# `arm-config.sh: No such file or directory` before reaching anything it is
# being tested for. That is precisely what happened on the first run, and it
# is the same trap `flake-inputs-parity-selftest.sh` records hitting.
for helper in arm-config.sh gate-ratchets.sh; do
  cp "$here/$helper" "$W/$helper" || exit 2
done

# Baseline. Without it every case below is satisfied by a gate failing for an
# unrelated reason.
echo "== baseline =="
if bash "$GATE" "$BIN" "$REAL" > "$W/baseline.log" 2>&1; then
  echo "  ok       the unperturbed gate passes: $(grep -a '^RESULT' "$W/baseline.log")"
else
  echo "  WRONG    the unperturbed gate already fails, so no case below means anything:"
  tail -8 "$W/baseline.log" | sed 's/^/           /'
  exit 1
fi

# Run the gate against a wrapper and require it to fail naming `want`.
run_case() { # NAME WRAPPER-PATH EXPECTED-SUBSTRING
  local name=$1 wrapper=$2 want=$3 rc
  checked=$((checked + 1))
  chmod +x "$wrapper"
  bash "$GATE" "$BIN" "$wrapper" > "$W/$name.log" 2>&1
  rc=$?
  if [ "$rc" -eq 0 ]; then
    echo "  WRONG    $name: the gate PASSED against a driver broken on purpose"
    fails=$((fails + 1))
  elif LC_ALL=C grep -aqF "$want" "$W/$name.log"; then
    printf '  ok       %-14s exit %s, named it: %s\n' "$name" "$rc" "$want"
  else
    echo "  WRONG    $name: failed (exit $rc) but not with '$want':"
    LC_ALL=C grep -aE 'MISMATCH|^RESULT|rust-driver-parity:' "$W/$name.log" | head -3 | sed 's/^/           /'
    fails=$((fails + 1))
  fi
}

# Shared preamble for the wrappers: find --store-root and the printed path.
# Written into each wrapper rather than sourced, so a wrapper is one
# self-contained file the gate can be handed.
# Deliberately single-quoted: this text is pasted INTO the wrappers and must
# expand when they run, not when this file builds them (SC2016 is reporting
# that it is doing its job).
# shellcheck disable=SC2016
preamble='
root=""; prev=""
for a in "$@"; do
  [ "$prev" = --store-root ] && root=$a
  prev=$a
done
'

echo "== cases =="

# 1. The sharpest one: the right path, the wrong bytes.
#
# Sharper than corrupting the input, which the crate catches by itself --
# `NeedPath::WriteDrv` carries the path the evaluator computed and refuses an
# answer that disagrees, so perturbing the ATerm before the write trips THAT
# guard and never reaches the gate's byte comparison. This writes the correct
# path and then changes the file, which only the three-root byte check can see.
cat > "$W/w-bytes" <<EOF
#!/usr/bin/env bash
$preamble
out=\$("$REAL" "\$@"); rc=\$?
[ \$rc -eq 0 ] || { printf '%s\n' "\$out"; exit \$rc; }
printf '%s\n' "\$out"
f="\$root\$out"
[ -n "\$root" ] && [ -f "\$f" ] && { chmod u+w "\$f"; printf ' ' >> "\$f"; }
exit 0
EOF
run_case bytes "$W/w-bytes" "drv-bytes(driver="

# 2. The right path and no file at all -- the shape ENG-12799 was, where an
# arm printed a correct drvPath and left nothing behind it.
cat > "$W/w-missing" <<EOF
#!/usr/bin/env bash
$preamble
out=\$("$REAL" "\$@"); rc=\$?
[ \$rc -eq 0 ] || { printf '%s\n' "\$out"; exit \$rc; }
printf '%s\n' "\$out"
[ -n "\$root" ] && { chmod -R u+w "\$root" 2>/dev/null; rm -f "\$root\$out"; }
exit 0
EOF
run_case missing "$W/w-missing" "no-drv-written(driver)"

# 3. A divergent drvPath. One character of the hash, which is the smallest
# difference that is still a different store path.
cat > "$W/w-path" <<EOF
#!/usr/bin/env bash
out=\$("$REAL" "\$@"); rc=\$?
[ \$rc -eq 0 ] || { printf '%s\n' "\$out"; exit \$rc; }
case "\$1" in
  instantiate) printf '%s\n' "\$(printf '%s' "\$out" | sed 's|/nix/store/.|/nix/store/z|')" ;;
  *) printf '%s\n' "\$out" ;;
esac
exit 0
EOF
run_case drvpath "$W/w-path" "drvPath(driver="

# 4. The outPath arm fails outright.
#
# This is the case the arm could not see at all before review: all three
# commands discarded stderr and none of the three exit codes was checked, so
# three arms that ALL failed left three empty strings, both comparisons held,
# and the case scored a match having measured nothing.
# Only the `.outPath` evaluation, NOT every `eval`. The first version of this
# wrapper failed all of them, so the gate died at its own capability probe
# ("the driver cannot evaluate the probe expression '1'") and never reached
# the arm under test -- a perturbed run that failed for the wrong reason,
# which is the second of the three WRONG cases above and the reason this file
# checks the message rather than only the exit code.
cat > "$W/w-outpath" <<EOF
#!/usr/bin/env bash
for a in "\$@"; do
  case "\$a" in
    *.outPath*) echo "selftest: the outPath arm is broken on purpose" >&2; exit 1 ;;
  esac
done
exec "$REAL" "\$@"
EOF
run_case outpath "$W/w-outpath" "outPath-failed(driver"

# 5. A driver told the wrong system.
#
# The case that found a hole rather than confirming one. It passed 19/19
# against the gate as first written, because nothing in the corpus read
# `builtins.currentSystem`; the two cases that do are what make it fail now.
# If this case ever passes again, somebody has removed them.
cat > "$W/w-system" <<EOF
#!/usr/bin/env bash
args=(); prev=""
for a in "\$@"; do
  if [ "\$prev" = --system ]; then args+=("\${a}-SELFTEST"); else args+=("\$a"); fi
  prev=\$a
done
exec "$REAL" "\${args[@]}"
EOF
run_case system "$W/w-system" "MISMATCH"

# 6. An emptied corpus must not print `RESULT ... pass`.
#
# The gate's own source, not a wrapper, because what this asserts is about
# reporting. `verdict` used to come from `mismatch` alone, so a corpus that
# had silently shrunk to nothing printed the word `pass` and then exited 1
# from the ratchet -- and a log scraper keying on the RESULT line reads the
# word, not the exit code.
checked=$((checked + 1))
python3 -c '
import re, sys
src, dst = sys.argv[1:3]
s = open(src).read()
out, n = re.subn(r"declare -a CASES=\(.*?\n\)\n", "declare -a CASES=()\n", s, count=1, flags=re.S)
if n != 1:
    sys.exit("could not empty the CASES array; the gate has been restructured")
open(dst, "w").write(out)
' "$GATE" "$W/gate-empty.sh" || { echo "  WRONG    emptycorpus: could not perturb the gate"; fails=$((fails + 1)); }
if [ -f "$W/gate-empty.sh" ]; then
  bash "$W/gate-empty.sh" "$BIN" "$REAL" > "$W/emptycorpus.log" 2>&1
  rc=$?
  line=$(grep -a '^RESULT rust-driver-parity ' "$W/emptycorpus.log" | head -1)
  # The RESULT line must be PRESENT and say fail. Requiring only "does not say
  # pass" is satisfied by no RESULT line at all -- a gate that died before
  # reporting -- which is the absence-as-success shape this whole file exists
  # to refuse.
  if [ "$rc" -eq 0 ]; then
    echo "  WRONG    emptycorpus: a gate with no cases exited 0"
    fails=$((fails + 1))
  elif [ -z "$line" ]; then
    echo "  WRONG    emptycorpus: exited $rc without printing a RESULT line at all:"
    tail -3 "$W/emptycorpus.log" | sed 's/^/           /'
    fails=$((fails + 1))
  elif printf '%s' "$line" | grep -q 'rust-driver-parity fail cases=0 '; then
    printf '  ok       %-14s exit %s, RESULT says: %s\n' emptycorpus "$rc" "$(printf '%s' "$line" | cut -d' ' -f1-5)"
  else
    echo "  WRONG    emptycorpus: exited $rc but the RESULT line is not 'fail cases=0': $line"
    fails=$((fails + 1))
  fi
fi

echo
echo "RESULT rust-driver-parity-selftest checked=$checked wrong=$fails gate=$GATE driver=$REAL"
[ "$fails" -eq 0 ] || {
  echo "rust-driver-parity-selftest: $fails case(s) wrong. The gate does not detect something it claims to."
  exit 1
}
exit 0
