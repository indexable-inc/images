#!/usr/bin/env bash
# Paired A/B evaluator benchmark: cpuTime of two `nix-instantiate` binaries.
#
#   positions-bench.sh REPO BEFORE_BIN AFTER_BIN BEFORE_REV AFTER_REV
#
# BEFORE_BIN and AFTER_BIN must each come from their OWN complete build tree.
# Swapping just `src/nix/nix` between trees pairs it with the other revision's
# libnixexpr and it dies in dyld (ENG-13097).
#
# Rounds default to 5 on the drv-parity corpus and 12 on the short arms;
# override with DR= and HR=. Read the median of the per-round RATIO.
#
# Two complete build trees, sampled INTERLEAVED.
#
# Sequential arm-after-arm pairs disagreed by more than the effect being
# measured (drv corpus -0.3% and +2.4% on the same pair of binaries), because
# each pair measured its arms minutes apart on a Mac with other agents on it,
# so machine drift aliased with the arm. Alternating per sample cancels drift:
# the statistic to read is the median of the per-round RATIO, not the ratio of
# the two medians.
set -u
root=$1; NA=$2; NB=$3; reva=$4; revb=$5
export NIX_USER_CONF_FILES=/dev/null
# A nixpkgs to import for the one non-synthetic arm. Any checkout works; the
# arms are compared against each other, not against a published number, so the
# only requirement is that both binaries see the same one.
NP=${NIXPKGS_FOR_BENCH:-/nix/store/llgwlxshmy0ifvxh7f8wq53vk5x7vd13-source}
RUST='extra-experimental-features = rust-eval nix-command flakes
lint-url-literals = ignore
lint-short-path-literals = ignore
lint-absolute-path-literals = ignore
system = x86_64-linux
eval-backend = rust'
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
# A sample runs inside $(...), so an `exit` there ends the SUBSHELL and the
# harness carries on with whatever the substitution produced. Signalling the
# script itself is what actually stops it.
trap 'echo "bench: a sample failed, refusing to report" >&2; exit 3' TERM
mkdir -p "$W/src"; echo "hello from a source file" > "$W/src/f.txt"

mapfile -t CASES < <(python3 - "$root/maintainers/ix/drv-parity.sh" <<'PY'
import sys
text = open(sys.argv[1]).read()
i = text.index('(', text.index('declare -a CASES=('))
depth = 0
for j in range(i, len(text)):
    if text[j] == '(': depth += 1
    elif text[j] == ')':
        depth -= 1
        if depth == 0: break
for line in text[i+1:j].splitlines():
    line = line.strip()
    if line and not line.startswith('#'):
        print(line)
PY
)
[ "${#CASES[@]}" -gt 0 ] || { echo "no drv-parity cases parsed" >&2; exit 2; }

die() { echo "bench: $1" >&2; sed -n 1,5p "$W/err" >&2; kill -TERM $$; exit 3; }
one() { # BIN EXPR -> cpuTime
  # Deleted first. Left in place, a binary that dies before writing stats hands
  # back the PREVIOUS sample -- which is the other arm's -- and the harness
  # prints a plausible table for a binary that never ran. Watched happen: with
  # /usr/bin/false as one arm this printed a full table and exited 0.
  rm -f "$W/s.json"
  # A nonzero exit is NOT the failure signal: the drv-parity corpus contains
  # expressions that are meant to fail, and cppnix writes the stats file anyway.
  # Absent stats is the signal, and it is the one a dyld abort or a missing
  # binary gives.
  NIX_CONFIG="$RUST" NIX_SHOW_STATS=1 NIX_SHOW_STATS_PATH="$W/s.json" \
    "$1" --eval --strict -E "$2" > "$W/out" 2> "$W/err"
  [ -f "$W/s.json" ] || die "$1 wrote no stats -- the run died"
  python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["cpuTime"])' "$W/s.json" \
    || die "$1 wrote unreadable stats"
}
drv_pass() { local bin=$1 c cpu=0 r
  for c in "${CASES[@]}"; do
    c=${c#\"}; c=${c%\"}; c=${c//\\\"/\"}
    c=${c//\$D/name = \"g\"; system = \"x86_64-linux\"; builder = \"/bin/sh\";}
    c=${c//\$W/$W}
    r=$(one "$bin" "$c") || exit 3; cpu=$(python3 -c "print($cpu + $r)")
  done; echo "$cpu"; }
SYNTH="builtins.foldl' (acc: i: acc + (let s = { a = i; b = i + 1; c = \"s\${toString i}\"; }; in s.a + s.b + builtins.stringLength s.c)) 0 (builtins.genList (x: x) 400000)"
ARITH="builtins.foldl' (acc: i: acc + i * 2 + (if i > 3 then 1 else 0)) 0 (builtins.genList (x: x) 400000)"
HELLO="(import $NP {}).hello.drvPath"

echo "warming ($(date +%H:%M:%S))"
for b in "$NA" "$NB"; do one "$b" "$HELLO" >/dev/null; one "$b" "$SYNTH" >/dev/null; drv_pass "$b" >/dev/null; done

declare -a DA=() DB=() HA=() HB=() SA=() SB=() AA=() AB=()
DR=${DR:-5}; HR=${HR:-12}
for r in $(seq 1 "$DR"); do
  if [ $((r % 2)) -eq 1 ]; then DA+=("$(drv_pass "$NA")"); DB+=("$(drv_pass "$NB")")
  else                          DB+=("$(drv_pass "$NB")"); DA+=("$(drv_pass "$NA")"); fi
  echo "  drv round $r done ($(date +%H:%M:%S))"
done
for r in $(seq 1 "$HR"); do
  if [ $((r % 2)) -eq 1 ]; then
    HA+=("$(one "$NA" "$HELLO")"); HB+=("$(one "$NB" "$HELLO")")
    SA+=("$(one "$NA" "$SYNTH")"); SB+=("$(one "$NB" "$SYNTH")")
    AA+=("$(one "$NA" "$ARITH")"); AB+=("$(one "$NB" "$ARITH")")
  else
    HB+=("$(one "$NB" "$HELLO")"); HA+=("$(one "$NA" "$HELLO")")
    SB+=("$(one "$NB" "$SYNTH")"); SA+=("$(one "$NA" "$SYNTH")")
    AB+=("$(one "$NB" "$ARITH")"); AA+=("$(one "$NA" "$ARITH")")
  fi
done

python3 - "$reva" "$revb" "${DA[*]}" "${DB[*]}" "${HA[*]}" "${HB[*]}" "${SA[*]}" "${SB[*]}" "${AA[*]}" "${AB[*]}" <<'PY'
import statistics, sys
reva, revb = sys.argv[1], sys.argv[2]
xs = [[float(v) for v in s.split()] for s in sys.argv[3:11]]
names = ["drv-parity-corpus (60)", "nixpkgs-hello", "synthetic-attrs", "synthetic-arith"]
print(f"PAIRED before={reva} after={revb}")
print(f"{'arm':<26}{'before':>9}{'after':>9}{'ratio-med':>11}{'delta':>9}  n")
for k, name in enumerate(names):
    a, b = xs[2*k], xs[2*k+1]
    n = min(len(a), len(b))
    ratios = sorted(b[i]/a[i] for i in range(n))
    rm = statistics.median(ratios)
    print(f"{name:<26}{statistics.median(a):>8.3f}s{statistics.median(b):>8.3f}s"
          f"{rm:>11.4f}{(rm-1)*100:>+8.2f}%  {n}")
    print(f"{'':<26}per-round ratios {min(ratios):.4f}-{max(ratios):.4f}")
PY
