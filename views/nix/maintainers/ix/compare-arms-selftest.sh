#!/usr/bin/env bash
#
# The scorer's own guard. A guard you have not watched fail is not a guard,
# and this one exists to stop a class of mistake that has been made four
# times, so it gets checked rather than trusted.
#
# Runs in under a second and needs no nix, no store and no built binary: the
# scorer takes files and exit codes, so the whole matrix is `printf`.
set -u
cd "$(dirname "$0")" || exit 2
# shellcheck source=./compare-arms.sh
. ./compare-arms.sh

W=$(mktemp -d) || exit 2
trap 'rm -rf "$W"' EXIT
fails=0
checked=0

want() { # EXPECTED CPP_TEXT CPP_RC RUST_TEXT RUST_RC WHY
  local expected=$1 cpp=$2 cpp_rc=$3 rust=$4 rust_rc=$5 why=$6
  printf '%s' "$cpp" > "$W/cpp"
  printf '%s' "$rust" > "$W/rust"
  ARMS_VERDICT=
  arms_score "$W/cpp" "$cpp_rc" "$W/rust" "$rust_rc"
  checked=$((checked + 1))
  if [ "$ARMS_VERDICT" = "$expected" ]; then
    printf '  ok       %-11s %s\n' "$ARMS_VERDICT" "$why"
  else
    printf '  WRONG    got %-8s want %-8s %s\n' "$ARMS_VERDICT" "$expected" "$why"
    fails=$((fails + 1))
  fi
}

echo "== arms_score =="
want match     'v' 0 'v' 0 "both said the same thing, and said something"
# The four-times bug. `rc_a == rc_b && cmp -s a b` is true here.
want empty     ''  0 ''  0 "both exited 0 and printed nothing"
want fail-both ''  1 ''  1 "both failed the same way; the caller decides"
want differ    'v' 0 ''  0 "one arm printed nothing and the other did not"
want differ    'a' 1 'b' 1 "both failed, differently, which is a divergence"
want differ    'v' 0 'v' 1 "same bytes, different exit codes"
want differ    'a' 0 'b' 0 "different bytes"

echo "== arms_require_rows =="
# Run in a subshell because the refusal is an exit, and take the status
# directly rather than through a pipe -- a `| sed` here would report sed's
# status and this file would be asserting nothing, which is the same shape it
# is guarding against.
( arms_require_rows 0 "rows" ) > "$W/out" 2>&1
rc=$?
checked=$((checked + 1))
if [ "$rc" -eq 2 ] && grep -q "measured nothing" "$W/out"; then
  echo "  ok       refused a zero-row run (exit $rc)"
else
  echo "  WRONG    a zero-row run exited $rc and said: $(head -1 "$W/out")"
  fails=$((fails + 1))
fi

( arms_require_rows 3 "rows" ) > "$W/out" 2>&1
rc=$?
checked=$((checked + 1))
if [ "$rc" -eq 0 ]; then
  echo "  ok       let a three-row run through"
else
  echo "  WRONG    a three-row run exited $rc"
  fails=$((fails + 1))
fi

echo
echo "RESULT compare-arms-selftest checked=$checked failed=$fails"
[ "$fails" -eq 0 ]
